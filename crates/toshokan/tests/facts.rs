//! Facts: how entries merge, what a view shows, how intents become ops, and undo.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use toshokan::binding::Bindings;
use toshokan::env::SeededRandom;
use toshokan::intent::{ops, Driver, Intent};
use toshokan::line::Line;
use toshokan::log::{Displaced, Entry, EntryKind, FileFact, Logged, Op};
use toshokan::merge::Folded;
use toshokan::plan::{Expect, FileChange, Plan};
use toshokan::undo::History;
use toshokan::view::{Conflicted, Parts, View};
use toshokan::{
    EntityId, EntryHash, Field, Hlc, Identity, Invalid, Nonce, Random, Raw, Refusal, Register,
    RelPath, Schema, Set, WriterId,
};

const NAME: Register<String> = Register::new("name");
const NOTE: Register<Option<String>> = Register::new("note");
const TAGS: Set<String> = Set::new("tags");

fn schema() -> Schema {
    Schema::of(&[NAME.key(), NOTE.key(), TAGS.key()]).unwrap()
}

/// A change an intent makes to an entity.
type Change = fn(Intent<Ids>, EntityId) -> Intent<Ids>;

/// Builds intents without a library: the entities they create take these ids.
struct Ids(Vec<EntityId>);

impl Driver for Ids {
    type Source = Vec<u8>;

    fn bytes(bytes: Vec<u8>) -> Vec<u8> {
        bytes
    }

    fn entity_id(&mut self) -> EntityId {
        self.0.remove(0)
    }
}

fn entity(id: u128) -> EntityId {
    EntityId::from_u128(id)
}

fn intent(label: &str) -> Intent<Ids> {
    creating(label, &[])
}

fn creating(label: &str, ids: &[EntityId]) -> Intent<Ids> {
    Intent::new(Ids(ids.to_vec()), label)
}

fn plan(intent: Intent<Ids>) -> Plan {
    intent.into_parts().1.unwrap()
}

fn view(folded: Folded) -> View {
    View::new(Parts {
        folded,
        bindings: Bindings::default(),
        writers: Vec::new(),
        forks: Vec::new(),
        gaps: Vec::new(),
    })
}

/// A snapshot's state as another reader decodes it.
fn reread(folded: &Folded) -> Folded {
    serde_json::from_str(&serde_json::to_string(folded).unwrap()).unwrap()
}

/// One instance writing as `id`, with what it has seen merged in `folded`.
#[derive(Clone)]
struct Writer {
    id: WriterId,
    /// Keeps the lines of a cloned history apart from the original's.
    salt: u64,
    head: EntryHash,
    clock: Hlc,
    folded: Folded,
    own: Vec<Entry>,
}

impl Writer {
    fn new(id: u128) -> Self {
        Self {
            id: WriterId::from_u128(id),
            salt: 0,
            head: EntryHash::ZERO,
            clock: Hlc::ZERO,
            folded: Folded::default(),
            own: Vec::new(),
        }
    }

    fn log(&mut self, now: u64, kind: EntryKind) -> Entry {
        self.clock = self.clock.tick(now).unwrap();
        let json = format!(
            r#"{{"prev":"{}","writer":"{}","salt":{},"n":{}}}"#,
            self.head,
            self.id,
            self.salt,
            self.own.len()
        );
        let entry = Entry {
            line: Line::seal(json).unwrap(),
            at: self.clock,
            kind,
        };
        self.head = entry.hash();
        self.folded.apply(self.id, &entry);
        self.own.push(entry.clone());
        entry
    }

    fn intent(&mut self, now: u64, ops: Vec<Op>, displaced: Vec<Displaced>) -> Entry {
        let logged = Logged {
            label: "test".into(),
            ops,
            displaced,
            reverses: None,
        };
        self.log(now, EntryKind::Intent(logged))
    }

    fn commit(&mut self, plan: Plan, now: u64) -> Result<Entry, Invalid> {
        let ops = ops(&plan, &self.folded, &schema())?;
        let logged = Logged {
            label: plan.label,
            ops,
            displaced: Vec::new(),
            reverses: plan.reverses,
        };
        Ok(self.log(now, EntryKind::Intent(logged)))
    }

    fn receive(&mut self, writer: WriterId, entry: &Entry) {
        self.clock = self.clock.observe(entry.at);
        self.folded.apply(writer, entry);
    }

    fn sync(&mut self, from: &Writer) {
        for entry in &from.own {
            self.receive(from.id, entry);
        }
    }

    fn view(&self) -> View {
        view(self.folded.clone())
    }

    fn undo(&mut self, now: u64) -> Result<Entry, Refusal> {
        let plan = History::of(&self.own).plan_undo(&self.own, self.id, &self.view())?;
        Ok(self.commit(plan, now).unwrap())
    }

    fn redo(&mut self, now: u64) -> Result<Entry, Refusal> {
        let plan = History::of(&self.own).plan_redo(&self.own, self.id, &self.view())?;
        Ok(self.commit(plan, now).unwrap())
    }
}

/// Two writers who have both seen `e` created by the first with `name` "first"
/// and tag "a".
fn pair(e: EntityId) -> (Writer, Writer) {
    let mut a = Writer::new(0xa);
    let mut b = Writer::new(0xb);
    let mut create = creating("Add", &[e]);
    create.create(|new| {
        new.set(NAME, "first".into()).add(TAGS, "a".into());
    });
    a.commit(plan(create), 1).unwrap();
    b.sync(&a);
    (a, b)
}

fn name(writer: &Writer, e: EntityId) -> Field<String> {
    writer.view().entity(e).unwrap().get(NAME)
}

fn tags(writer: &Writer, e: EntityId) -> Vec<String> {
    writer.view().entity(e).unwrap().members(TAGS).values
}

#[test]
fn concurrent_different_values_conflict_and_the_latest_is_shown() {
    let e = entity(1);
    let (mut a, mut b) = pair(e);
    let left = a
        .commit(plan(intent("").set(e, NAME, "left".into())), 3)
        .unwrap();
    let right = b
        .commit(plan(intent("").set(e, NAME, "right".into())), 2)
        .unwrap();
    a.sync(&b);

    let field = name(&a, e);
    let Field::Conflict { shown, all } = &field else {
        panic!("{field:?}");
    };
    assert_eq!(
        shown, "left",
        "the later clock is shown over the higher writer"
    );
    let all: Vec<_> = all
        .iter()
        .map(|w| (w.value.as_str(), w.by, w.entry))
        .collect();
    assert_eq!(
        all,
        [("right", b.id, right.hash()), ("left", a.id, left.hash())]
    );
    let conflict = Conflicted::Field {
        entity: e,
        key: "name".into(),
    };
    assert_eq!(a.view().conflicts(), [conflict]);
}

#[test]
fn the_writer_id_breaks_a_clock_tie() {
    let e = entity(1);
    let (mut a, mut b) = pair(e);
    a.commit(plan(intent("").set(e, NAME, "left".into())), 2)
        .unwrap();
    b.commit(plan(intent("").set(e, NAME, "right".into())), 2)
        .unwrap();
    a.sync(&b);
    assert_eq!(name(&a, e).shown().map(String::as_str), Some("right"));
}

#[test]
fn equal_concurrent_values_are_not_a_conflict() {
    let e = entity(1);
    let (mut a, mut b) = pair(e);
    a.commit(plan(intent("").set(e, NAME, "same".into())), 2)
        .unwrap();
    b.commit(plan(intent("").set(e, NAME, "same".into())), 2)
        .unwrap();
    a.sync(&b);
    assert_eq!(name(&a, e), Field::Value("same".into()));
    assert_eq!(a.view().conflicts(), []);
}

#[test]
fn a_write_after_seeing_a_conflict_resolves_it_for_every_reader() {
    let e = entity(1);
    let (mut a, mut b) = pair(e);
    a.commit(plan(intent("").set(e, NAME, "left".into())), 2)
        .unwrap();
    b.commit(plan(intent("").set(e, NAME, "right".into())), 2)
        .unwrap();
    a.sync(&b);
    a.commit(plan(intent("").set(e, NAME, "chosen".into())), 3)
        .unwrap();
    b.sync(&a);
    for reader in [&a, &b] {
        assert_eq!(name(reader, e), Field::Value("chosen".into()));
        assert_eq!(reader.view().conflicts(), []);
    }
}

#[test]
fn a_clear_concurrent_with_a_value_leaves_the_value() {
    let e = entity(1);
    let (mut a, mut b) = pair(e);
    a.commit(plan(intent("").clear(e, NAME)), 2).unwrap();
    assert_eq!(name(&a, e), Field::Unset);
    b.commit(plan(intent("").set(e, NAME, "kept".into())), 2)
        .unwrap();
    a.sync(&b);
    assert_eq!(name(&a, e), Field::Value("kept".into()));
}

#[test]
fn a_json_null_value_is_not_a_clear_in_a_snapshot() {
    let e = entity(1);
    let (mut a, _) = pair(e);
    a.commit(plan(intent("").set(e, NOTE, None)), 2).unwrap();
    let note = |writer: &Writer| view(reread(&writer.folded)).entity(e).unwrap().get(NOTE);
    assert_eq!(note(&a), Field::Value(None));
    a.commit(plan(intent("").clear(e, NOTE)), 3).unwrap();
    assert_eq!(note(&a), Field::Unset);
}

#[test]
fn a_concurrent_add_survives_a_remove_of_the_same_value() {
    let e = entity(1);
    let (mut a, mut b) = pair(e);
    a.commit(plan(intent("").remove(e, TAGS, &"a".into())), 2)
        .unwrap();
    assert_eq!(tags(&a, e), Vec::<String>::new());
    b.commit(plan(intent("").add(e, TAGS, "a".into())), 2)
        .unwrap();
    a.sync(&b);
    assert_eq!(tags(&a, e), ["a"]);
}

#[test]
fn removing_one_value_keeps_another_the_same_intent_added() {
    let e = entity(1);
    let (mut a, _) = pair(e);
    let both = intent("").add(e, TAGS, "x".into()).add(e, TAGS, "y".into());
    a.commit(plan(both), 2).unwrap();
    a.commit(plan(intent("").remove(e, TAGS, &"x".into())), 3)
        .unwrap();
    assert_eq!(tags(&a, e), ["a", "y"]);
    assert_eq!(a.view().find(TAGS, &"y".into()), [e]);
    assert_eq!(a.view().find(TAGS, &"x".into()), []);
}

#[test]
fn a_delete_ends_an_entity_whose_writes_it_observed() {
    let e = entity(1);
    let deletes = [
        intent("").delete(e),
        intent("").add(e, TAGS, "same intent".into()).delete(e),
    ];
    for delete in deletes {
        let (mut a, mut b) = pair(e);
        a.commit(plan(delete), 2).unwrap();
        b.sync(&a);
        for reader in [&a, &b] {
            assert!(reader.view().entity(e).is_none());
            assert_eq!(reader.view().conflicts(), []);
        }
    }
}

#[test]
fn a_write_the_delete_did_not_observe_keeps_the_entity_in_conflict() {
    let e = entity(1);
    let cases: [(&str, Change); 3] = [
        ("an add", |i, e| i.add(e, TAGS, "late".into())),
        ("a set", |i, e| i.set(e, NAME, "late".into())),
        ("a revive", |i, e| i.revive(e)),
    ];
    for (what, write) in cases {
        let (mut a, mut b) = pair(e);
        a.commit(plan(intent("").delete(e)), 2).unwrap();
        b.commit(plan(write(intent(""), e)), 2).unwrap();
        a.sync(&b);
        let view = a.view();
        let shown = view.entity(e).unwrap_or_else(|| panic!("{what} lost"));
        assert!(shown.deletion_conflicted(), "{what}");
        assert_eq!(
            view.conflicts(),
            [Conflicted::Existence { entity: e }],
            "{what}"
        );

        let mut deleted = a.clone();
        deleted.commit(plan(intent("").delete(e)), 3).unwrap();
        assert!(deleted.view().entity(e).is_none(), "{what}: deleting again");
        a.commit(plan(intent("").revive(e)), 3).unwrap();
        let revived = a.view();
        assert!(
            !revived.entity(e).unwrap().deletion_conflicted(),
            "{what}: revived"
        );
        assert_eq!(revived.conflicts(), [], "{what}: revived");
    }
}

#[test]
fn values_that_do_not_decode_are_unreadable_and_kept() {
    let e = entity(1);
    let (mut a, _) = pair(e);
    let number = Raw::new("42").unwrap();
    let ops = vec![
        Op::Write {
            entity: e,
            key: "name".into(),
            value: Some(number.clone()),
            replaces: a
                .folded
                .register(e, "name")
                .iter()
                .map(|w| w.entry)
                .collect(),
        },
        Op::Add {
            entity: e,
            key: "tags".into(),
            value: number.clone(),
        },
    ];
    a.intent(2, ops, Vec::new());
    for folded in [a.folded.clone(), reread(&a.folded)] {
        let view = view(folded);
        let shown = view.entity(e).unwrap();
        assert_eq!(shown.get(NAME), Field::Unreadable(number.clone()));
        let members = shown.members(TAGS);
        assert_eq!(members.values, ["a"]);
        assert_eq!(members.unreadable, std::slice::from_ref(&number));
    }
}

#[test]
fn ops_refuse_what_the_schema_or_the_view_does_not_allow() {
    let e = entity(1);
    let (a, _) = pair(e);
    let other: Register<String> = Register::new("other");
    let misnamed: Set<String> = Set::new("name");
    let unencodable: Register<BTreeMap<Vec<u8>, u8>> = Register::new("name");
    let gone = entity(2);
    let cases = [
        (
            intent("").set(e, other, "x".into()).into_parts().1,
            Invalid::UndeclaredKey("other".into()),
        ),
        (
            intent("").add(e, misnamed, "x".into()).into_parts().1,
            Invalid::UndeclaredKey("name".into()),
        ),
        (
            intent("").set(gone, NAME, "x".into()).into_parts().1,
            Invalid::NoEntity(gone),
        ),
        (
            intent("").delete(gone).into_parts().1,
            Invalid::NoEntity(gone),
        ),
        (
            intent("").revive(gone).into_parts().1,
            Invalid::NoEntity(gone),
        ),
        (intent("").into_parts().1, Invalid::Empty),
    ];
    for (plan, refusal) in cases {
        let result = plan.and_then(|plan| ops(&plan, &a.folded, &schema()));
        assert_eq!(result, Err(refusal.clone()), "{refusal}");
    }
    let (_, plan, _) = intent("")
        .set(e, unencodable, BTreeMap::from([(vec![1], 1)]))
        .into_parts();
    assert!(matches!(plan, Err(Invalid::Unencodable { key, .. }) if key == "name"));
}

#[test]
fn ops_name_what_this_writer_observed() {
    let e = entity(1);
    let (mut a, mut b) = pair(e);
    let left = a
        .commit(plan(intent("").set(e, NAME, "left".into())), 2)
        .unwrap();
    let right = b
        .commit(plan(intent("").set(e, NAME, "right".into())), 2)
        .unwrap();
    a.sync(&b);
    let tag = a.folded.tags(e, "tags", &Raw::of(&"a").unwrap())[0].entry;

    let plan = plan(
        intent("")
            .set(e, NAME, "x".into())
            .set(e, NAME, "y".into())
            .delete(e),
    );
    let mut written = ops(&plan, &a.folded, &schema()).unwrap();
    let Some(Op::Delete { observed, .. }) = written.pop() else {
        panic!("{written:?}");
    };
    let mut expected = vec![left.hash(), right.hash(), tag];
    expected.sort();
    let mut observed = observed;
    observed.sort();
    assert_eq!(observed, expected);
    let [Op::Write {
        value, replaces, ..
    }] = written.as_slice()
    else {
        panic!("one write of a register per intent: {written:?}");
    };
    assert_eq!(value.as_ref().map(Raw::as_str), Some(r#""y""#));
    assert_eq!(replaces.len(), 2);
}

#[test]
fn compaction_never_resurrects_a_replaced_write() {
    let e = entity(1);
    let (mut a, mut b) = pair(e);
    b.commit(plan(intent("").set(e, NAME, "second".into())), 2)
        .unwrap();
    a.commit(plan(intent("").add(e, TAGS, "b".into())), 3)
        .unwrap();

    let mut own_a = Folded::default();
    for entry in &a.own {
        own_a.apply(a.id, entry);
    }
    let mut own_b = Folded::default();
    for entry in &b.own {
        own_b.apply(b.id, entry);
    }
    let mut a_compacted = reread(&own_a);
    for entry in &b.own {
        a_compacted.apply(b.id, entry);
    }
    let mut b_compacted = reread(&own_b);
    for entry in &a.own {
        b_compacted.apply(a.id, entry);
    }
    let mut both = reread(&own_a);
    both.join(&reread(&own_b));
    for folded in [a_compacted, b_compacted, both] {
        let reader = view(folded);
        assert_eq!(
            reader.entity(e).unwrap().get(NAME),
            Field::Value("second".into())
        );
    }
}

#[test]
fn undo_sets_a_field_back_and_redo_sets_it_again() {
    let e = entity(1);
    let (mut a, _) = pair(e);
    let rename = a
        .commit(plan(intent("Rename").set(e, NAME, "second".into())), 2)
        .unwrap();

    let undo = a.undo(3).unwrap();
    assert_eq!(name(&a, e), Field::Value("first".into()));
    let EntryKind::Intent(logged) = &undo.kind else {
        panic!();
    };
    assert_eq!(
        (logged.label.as_str(), logged.reverses),
        ("Rename", Some(rename.hash()))
    );
    let history = History::of(&a.own);
    let undone: Vec<_> = history
        .items()
        .iter()
        .map(|i| (i.label.as_str(), i.undone))
        .collect();
    assert_eq!(undone, [("Add", false), ("Rename", true)]);

    a.redo(4).unwrap();
    assert_eq!(name(&a, e), Field::Value("second".into()));
    assert!(!History::of(&a.own).items()[1].undone);
    a.undo(5).unwrap();
    a.undo(6).unwrap();
    assert!(a.view().entity(e).is_none(), "undoing the create deletes");
    assert_eq!(a.undo(7).unwrap_err(), Refusal::Nothing);
    a.redo(8).unwrap();
    assert_eq!(
        name(&a, e),
        Field::Value("first".into()),
        "redo revives it whole"
    );
    assert_eq!(tags(&a, e), ["a"]);
}

#[test]
fn a_new_intent_ends_redo() {
    let e = entity(1);
    let (mut a, _) = pair(e);
    a.commit(plan(intent("").set(e, NAME, "second".into())), 2)
        .unwrap();
    a.undo(3).unwrap();
    a.commit(plan(intent("").add(e, TAGS, "b".into())), 4)
        .unwrap();
    assert_eq!(a.redo(5).unwrap_err(), Refusal::Nothing);
}

#[test]
fn undo_and_redo_of_set_members() {
    let e = entity(1);
    let (mut a, _) = pair(e);
    a.commit(plan(intent("").add(e, TAGS, "b".into())), 2)
        .unwrap();
    a.commit(plan(intent("").remove(e, TAGS, &"a".into())), 3)
        .unwrap();
    assert_eq!(tags(&a, e), ["b"]);
    a.undo(4).unwrap();
    assert_eq!(tags(&a, e), ["a", "b"]);
    a.undo(5).unwrap();
    assert_eq!(tags(&a, e), ["a"]);
    a.redo(6).unwrap();
    assert_eq!(tags(&a, e), ["a", "b"]);
}

#[test]
fn undo_is_refused_where_another_writer_changed_the_thing_since() {
    let e = entity(1);
    let cases: [(&str, Change, Change); 6] = [
        (
            "a later set",
            |i, e| i.set(e, NAME, "mine".into()),
            |i, e| i.set(e, NAME, "theirs".into()),
        ),
        (
            "a later clear",
            |i, e| i.set(e, NAME, "mine".into()),
            |i, e| i.clear(e, NAME),
        ),
        (
            "a remove of the added value",
            |i, e| i.add(e, TAGS, "b".into()),
            |i, e| i.remove(e, TAGS, &"b".into()),
        ),
        (
            "an add of the same value",
            |i, e| i.add(e, TAGS, "b".into()),
            |i, e| i.add(e, TAGS, "b".into()),
        ),
        (
            "an add of the removed value",
            |i, e| i.remove(e, TAGS, &"a".into()),
            |i, e| i.add(e, TAGS, "a".into()),
        ),
        (
            "a delete",
            |i, e| i.set(e, NAME, "mine".into()),
            |i, e| i.delete(e),
        ),
    ];
    for (what, mine, theirs) in cases {
        // A remove of a value the remover never saw added does nothing.
        let concurrency: &[bool] = match what {
            "a remove of the added value" => &[false],
            _ => &[false, true],
        };
        for &concurrent in concurrency {
            let (mut a, mut b) = pair(e);
            a.commit(plan(mine(intent(""), e)), 2).unwrap();
            if !concurrent {
                b.sync(&a);
            }
            let change = b.commit(plan(theirs(intent(""), e)), 3).unwrap();
            a.sync(&b);
            let refusal = Refusal::ChangedSince {
                by: b.id,
                entry: change.hash(),
            };
            assert_eq!(a.undo(4), Err(refusal), "{what}, concurrent: {concurrent}");
        }
    }
}

#[test]
fn undoing_a_create_is_refused_once_another_writer_wrote_to_it() {
    let e = entity(1);
    let (mut a, mut b) = pair(e);
    let tag = b
        .commit(plan(intent("").add(e, TAGS, "b".into())), 2)
        .unwrap();
    a.sync(&b);
    let refusal = Refusal::ChangedSince {
        by: b.id,
        entry: tag.hash(),
    };
    assert_eq!(a.undo(3), Err(refusal));
}

#[test]
fn undo_puts_files_back_and_leaves_pins_alone() {
    let e = entity(1);
    let fact = |path: &str, identity: u128| FileFact {
        path: RelPath::new(path).unwrap(),
        identity: Identity::from_u128(identity),
        len: 1,
        modified: None,
    };
    let item = Nonce::from_u128(0x7);
    let displaced = |from: &str, identity: u128| Displaced {
        item,
        from: RelPath::new(from).unwrap(),
        identity: Identity::from_u128(identity),
        len: 1,
    };
    let saved = Some(fact("a.bin", 1));
    type Case = (
        &'static str,
        Option<Option<FileFact>>,
        Option<FileFact>,
        Vec<Displaced>,
        Vec<FileChange>,
    );
    let cases: [Case; 4] = [
        (
            "a new file",
            None,
            Some(fact("a.bin", 1)),
            vec![],
            vec![FileChange::Trash {
                entity: e,
                expect: Expect::Holds(Identity::from_u128(1)),
            }],
        ),
        (
            "a save over the file",
            Some(saved.clone()),
            Some(fact("a.bin", 2)),
            vec![displaced("a.bin", 1)],
            vec![FileChange::Restore {
                entity: e,
                item,
                to: RelPath::new("a.bin").unwrap(),
                expect: Expect::Holds(Identity::from_u128(2)),
            }],
        ),
        (
            "a rename",
            Some(saved.clone()),
            Some(fact("b.bin", 1)),
            vec![],
            vec![FileChange::Rename {
                entity: e,
                to: RelPath::new("a.bin").unwrap(),
                expect: Expect::Holds(Identity::from_u128(1)),
            }],
        ),
        (
            "a trash",
            Some(saved.clone()),
            None,
            vec![displaced("a.bin", 1)],
            vec![FileChange::Restore {
                entity: e,
                item,
                to: RelPath::new("a.bin").unwrap(),
                expect: Expect::Absent,
            }],
        ),
    ];
    for (what, before, after, trashed, expected) in cases {
        let (mut a, _) = pair(e);
        let mut replaces = Vec::new();
        if let Some(before) = before {
            let ops = vec![Op::File {
                entity: e,
                file: before,
                replaces: Vec::new(),
            }];
            replaces.push(a.intent(2, ops, Vec::new()).hash());
        }
        let file = Op::File {
            entity: e,
            file: after,
            replaces,
        };
        let pin = Op::Pin {
            entity: entity(9),
            file: fact("moved.bin", 3),
            replaces: Vec::new(),
        };
        let changed = a.intent(3, vec![file, pin], trashed);
        let plan = History::of(&a.own)
            .plan_undo(&a.own, a.id, &a.view())
            .unwrap();
        assert_eq!(plan.files, expected, "{what}");
        assert_eq!(plan.facts, [], "{what}");
        assert_eq!(plan.reverses, Some(changed.hash()), "{what}");
    }
}

/// JSON a newer writer might write: odd spacing, member order, big numbers and
/// escapes, which a reader must keep byte for byte.
fn json(random: &mut SeededRandom, depth: u32) -> String {
    let leaves = [
        "null",
        "true",
        "123456789012345678901234567890",
        "-1.5e300",
        "\"caf\\u00e9 \\\"quoted\\\"\"",
        "{ \"b\" : 1, \"a\" : [ ] }",
        "\"\"",
    ];
    let choice = (random.next_u128() % 10) as usize;
    match choice {
        7 if depth > 0 => format!("[{}, {}]", json(random, depth - 1), json(random, depth - 1)),
        8 if depth > 0 => format!(
            "{{\"z\":{},\"k{}\":{}}}",
            json(random, depth - 1),
            random.next_u128() % 100,
            json(random, depth - 1)
        ),
        _ => leaves[choice % leaves.len()].to_owned(),
    }
}

#[test]
fn unknown_kinds_ops_keys_and_values_are_kept_through_merges_and_snapshots() {
    let e = entity(1);
    for seed in 0..200 {
        let mut random = SeededRandom::new(seed);
        let (mut a, _) = pair(e);
        let unknown_kind = Raw::new(&format!(
            "{{\"kind\":\"future\",\"x\":{}}}",
            json(&mut random, 2)
        ))
        .unwrap();
        let unknown_op = Raw::new(&format!(
            "{{\"op\":\"future\",\"x\":{}}}",
            json(&mut random, 2)
        ))
        .unwrap();
        let undeclared = Raw::new(&json(&mut random, 2)).unwrap();
        let mut value = json(&mut random, 2);
        while value.starts_with('"') {
            value = json(&mut random, 2);
        }
        let unreadable = Raw::new(&value).unwrap();

        let kind = a.log(2, EntryKind::Unknown(unknown_kind.clone()));
        let replaces = a
            .folded
            .register(e, "name")
            .iter()
            .map(|w| w.entry)
            .collect();
        let ops = vec![
            Op::Unknown(unknown_op.clone()),
            Op::Write {
                entity: e,
                key: "future".into(),
                value: Some(undeclared.clone()),
                replaces: Vec::new(),
            },
            Op::Write {
                entity: e,
                key: "name".into(),
                value: Some(unreadable.clone()),
                replaces,
            },
        ];
        let op = a.intent(3, ops, Vec::new());

        let member = json(&mut random, 2);
        let text = serde_json::to_string(&a.folded).unwrap();
        let extended = format!("{{\"future\":{member},{}", &text[1..]);
        let mut other = Folded::default();
        other.join(&serde_json::from_str(&extended).unwrap());
        for (how, folded) in [
            ("applied", a.folded.clone()),
            ("reread", reread(&a.folded)),
            ("joined", other.clone()),
        ] {
            let unknown: Vec<_> = folded.unknown().cloned().collect();
            let mut expected = vec![
                (kind.hash(), unknown_kind.clone()),
                (op.hash(), unknown_op.clone()),
            ];
            expected.sort();
            assert_eq!(unknown, expected, "seed {seed}, {how}");
            let kept: Vec<_> = folded
                .register(e, "future")
                .into_iter()
                .map(|w| w.value)
                .collect();
            assert_eq!(
                kept,
                std::slice::from_ref(&undeclared),
                "seed {seed}, {how}"
            );
            let field = view(folded).entity(e).unwrap().get(NAME);
            assert_eq!(
                field,
                Field::Unreadable(unreadable.clone()),
                "seed {seed}, {how}"
            );
        }
        let rewritten = serde_json::to_string(&other).unwrap();
        assert!(
            rewritten.contains(&format!("\"future\":{member}")),
            "seed {seed}: {rewritten}"
        );
        assert_eq!(reread(&other), other, "seed {seed}");
    }
}

const E: EntityId = EntityId::from_u128(0xe);

#[derive(Clone, Copy, Debug)]
enum Act {
    Set(&'static str),
    Clear,
    Add(&'static str),
    Remove(&'static str),
    Delete,
    Revive,
}

fn act(act: Act) -> Plan {
    let i = intent("");
    plan(match act {
        Act::Set(value) => i.set(E, NAME, value.into()),
        Act::Clear => i.clear(E, NAME),
        Act::Add(value) => i.add(E, TAGS, value.into()),
        Act::Remove(value) => i.remove(E, TAGS, &value.into()),
        Act::Delete => i.delete(E),
        Act::Revive => i.revive(E),
    })
}

/// A history being explored. `states` holds the merge of every subset of `log`,
/// indexed by bit mask, each checked against every order of arrival;
/// `snapshots` holds each instance's compactions of its own entries.
#[derive(Clone)]
struct Node {
    instances: Vec<Instance>,
    log: Vec<(WriterId, Entry)>,
    states: Vec<Arc<Folded>>,
    snapshots: Vec<(usize, Arc<Folded>)>,
    acts: Vec<String>,
}

#[derive(Clone)]
struct Instance {
    writer: Writer,
    /// The entries it has seen, as a mask over the log.
    known: usize,
    /// The entries it wrote.
    own: usize,
}

const MAX_INSTANCES: usize = 3;

impl Node {
    /// A writer has created `E`, and every instance starts having seen it.
    fn seeded() -> Self {
        let mut seed = Writer::new(0x5);
        let mut create = creating("", &[E]);
        create.create(|_| {});
        let entry = seed.commit(plan(create), 0).unwrap();
        let mut node = Self {
            instances: Vec::new(),
            log: Vec::new(),
            states: vec![Arc::default()],
            snapshots: Vec::new(),
            acts: Vec::new(),
        };
        node.push(seed.id, entry);
        node
    }

    fn all(&self) -> usize {
        (1 << self.log.len()) - 1
    }

    /// Appends an entry, merging it into every subset, and checks every order of
    /// arrival that ends with it.
    fn push(&mut self, writer: WriterId, entry: Entry) {
        let bit = 1 << self.log.len();
        self.log.push((writer, entry));
        let (writer, entry) = self.log.last().unwrap();
        for mask in 0..bit {
            let mut state = Folded::clone(&self.states[mask]);
            state.apply(*writer, entry);
            self.states.push(Arc::new(state));
        }
        for mask in bit..bit << 1 {
            for earlier in (0..self.log.len() - 1).filter(|i| mask & (1 << i) != 0) {
                let (by, last) = &self.log[earlier];
                let mut state = Folded::clone(&self.states[mask & !(1 << earlier)]);
                state.apply(*by, last);
                assert!(
                    state == *self.states[mask],
                    "{:?} in order ending {earlier}",
                    self.acts
                );
            }
        }
    }

    /// Every way the next entry can be written: by an instance, a new one or, with
    /// `clones`, a clone of the first; having first seen everything or not; doing
    /// each act.
    fn children(&self, alphabet: &[Act], clones: bool, now: u64) -> Vec<Node> {
        let mut starts: Vec<(usize, Option<Instance>)> =
            (0..self.instances.len()).map(|i| (i, None)).collect();
        if self.instances.len() < MAX_INSTANCES {
            let index = self.instances.len();
            let mut writer = Writer::new(0xa + index as u128);
            writer.receive(self.log[0].0, &self.log[0].1);
            let fresh = Instance {
                writer,
                known: 1,
                own: 0,
            };
            starts.push((index, Some(fresh)));
            if let (true, Some(first)) = (clones, self.instances.first()) {
                let mut clone = first.clone();
                clone.writer.salt = index as u64;
                clone.own = 0;
                starts.push((index, Some(clone)));
            }
        }
        let mut children = Vec::new();
        for (index, start) in starts {
            for sync in [false, true] {
                let mut node = self.clone();
                node.instances.extend(start.clone());
                let instance = &mut node.instances[index];
                if sync {
                    if instance.known == self.all() {
                        continue;
                    }
                    for (by, entry) in &self.log {
                        instance.writer.receive(*by, entry);
                    }
                    instance.known = self.all();
                }
                for &a in alphabet {
                    let folded = &node.instances[index].writer.folded;
                    if matches!(a, Act::Revive)
                        && !folded.deletion_conflicted(E)
                        && folded.present(E)
                    {
                        continue;
                    }
                    let mut writer = node.instances[index].writer.clone();
                    let Ok(entry) = writer.commit(act(a), now) else {
                        continue;
                    };
                    let EntryKind::Intent(logged) = &entry.kind else {
                        unreachable!();
                    };
                    if logged.ops.is_empty() {
                        continue;
                    }
                    let mut child = node.clone();
                    let bit = 1 << child.log.len();
                    let instance = &mut child.instances[index];
                    instance.writer = writer;
                    instance.known |= bit;
                    instance.own |= bit;
                    let (id, own) = (instance.writer.id, instance.own);
                    child.acts.push(format!(
                        "{index}{}{a:?}",
                        if sync { " after sync " } else { " " }
                    ));
                    child.push(id, entry);
                    let snapshot = Arc::new(reread(&child.states[own]));
                    child.snapshots.push((own, snapshot));
                    children.push(child);
                }
            }
        }
        children
    }

    /// Checks every split of the history into two deliveries, adopting what the
    /// whole shows beyond each part, and every compaction an instance made,
    /// delivered with the rest.
    fn check(&self) {
        let full = self.all();
        let oracle = &*self.states[full];
        for mask in (0..=full).filter(|mask| mask & 1 != 0) {
            let mut state = Folded::clone(&self.states[mask]);
            state.join(&self.states[full ^ mask]);
            assert!(
                &state == oracle,
                "{:?} delivered as {mask:b} and the rest",
                self.acts
            );
            let adopted = adopt(oracle, &self.states[mask]);
            assert_eq!(
                shows(&adopted),
                adopted_shows(oracle, &self.states[mask]),
                "{:?} adopted onto {mask:b}",
                self.acts
            );
        }
        let mut compacted = Folded::clone(&self.states[1]);
        for (own, snapshot) in &self.snapshots {
            let mut state = Folded::clone(snapshot);
            state.join(&self.states[full ^ own]);
            assert!(&state == oracle, "{:?} with {own:b} compacted", self.acts);
            state.join(oracle);
            assert!(
                &state == oracle,
                "{:?} with {own:b} compacted and delivered",
                self.acts
            );
            compacted.join(snapshot);
        }
        assert!(
            &compacted == oracle,
            "{:?} with every instance compacted",
            self.acts
        );
    }
}

/// What a state shows of `E`: whether it exists, and if so its name's values and
/// its tags.
fn shows(state: &Folded) -> (bool, BTreeSet<Raw>, Vec<Raw>) {
    if !state.present(E) {
        return (false, BTreeSet::new(), Vec::new());
    }
    let values = state.register(E, "name").into_iter().map(|w| w.value);
    (true, values.collect(), state.members(E, "tags"))
}

/// `kept` with one more entry, by a writer that saw everything, logging the ops
/// that make it show what `all` shows.
fn adopt(all: &Folded, kept: &Folded) -> Folded {
    let ops: Vec<Op> = all.beyond(kept).into_iter().map(|b| b.op).collect();
    let mut adopted = kept.clone();
    if !ops.is_empty() {
        let mut adopter = Writer::new(0xf);
        let entry = adopter.intent(100, ops, Vec::new());
        adopted.apply(adopter.id, &entry);
    }
    adopted
}

/// What adopting shows: what `all` shows, but of several name values `kept`
/// lacks, only the latest beside those `kept` shows too.
fn adopted_shows(all: &Folded, kept: &Folded) -> (bool, BTreeSet<Raw>, Vec<Raw>) {
    let (present, values, members) = shows(all);
    let held = shows(kept).1;
    if values.difference(&held).count() <= 1 {
        return (present, values, members);
    }
    let new = all.register(E, "name").into_iter();
    let new = new.filter(|w| !held.contains(&w.value));
    let latest = new.max_by_key(|w| (w.at, w.by)).unwrap().value;
    let mut values: BTreeSet<Raw> = values.intersection(&held).cloned().collect();
    values.insert(latest);
    (present, values, members)
}

/// Every history of `entries` entries after the seed by up to three instances,
/// the third possibly a clone of the first, over `alphabet`: each merged in every
/// order, split into every two deliveries, and with every compaction.
fn exhaust(alphabet: &[Act], clones: bool, entries: usize) {
    let split = 2.min(entries);
    let mut frontier = vec![Node::seeded()];
    for depth in 0..split {
        frontier = frontier
            .iter()
            .flat_map(|node| node.children(alphabet, clones, depth as u64 / 2 + 1))
            .collect();
    }
    let next = AtomicUsize::new(0);
    let checked = AtomicUsize::new(0);
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get());
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                while let Some(node) = frontier.get(next.fetch_add(1, Ordering::Relaxed)) {
                    let count = explore(node, alphabet, clones, split, entries);
                    checked.fetch_add(count, Ordering::Relaxed);
                }
            });
        }
    });
    assert!(
        checked.into_inner() > 0,
        "no history reached {entries} entries"
    );
}

fn explore(node: &Node, alphabet: &[Act], clones: bool, depth: usize, entries: usize) -> usize {
    if depth == entries {
        node.check();
        return 1;
    }
    node.children(alphabet, clones, depth as u64 / 2 + 1)
        .iter()
        .map(|child| explore(child, alphabet, clones, depth + 1, entries))
        .sum()
}

#[test]
fn concurrent_values_converge_over_every_history() {
    exhaust(&[Act::Set("x"), Act::Set("y")], false, 5);
}

#[test]
fn values_and_clears_converge_over_every_history() {
    exhaust(&[Act::Set("x"), Act::Clear], false, 5);
}

#[test]
fn set_members_converge_over_every_history_with_clones() {
    exhaust(&[Act::Add("x"), Act::Remove("x")], true, 5);
}

#[test]
fn existence_converges_over_every_history() {
    exhaust(&[Act::Set("x"), Act::Delete, Act::Revive], false, 5);
}

#[test]
fn deletes_and_set_members_converge_over_every_history() {
    exhaust(&[Act::Add("x"), Act::Remove("x"), Act::Delete], false, 5);
}
