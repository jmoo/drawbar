//! Facts: how entries merge, what a view shows, how intents become ops, and undo.

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use serde::{Deserialize, Serialize};
use toshokan::binding::Bindings;
use toshokan::contribution::Contribution;
use toshokan::derive::{derive, Seen, SeenSegment};
use toshokan::env::SeededRandom;
use toshokan::intent::{ops, Driver, Intent};
use toshokan::line::Line;
use toshokan::log::{Displaced, Entry, EntryKind, FileFact, Logged, Op, Ref};
use toshokan::merge::{Folded, Write};
use toshokan::plan::{Expect, FileChange, Plan};
use toshokan::snapshot::{Certified, Snapshot};
use toshokan::undo::{Before, Entering, History};
use toshokan::view::{Conflicted, Parts, View};
use toshokan::{
    EntityId, EntryHash, Field, Hlc, Identity, Invalid, Nonce, Random, Raw, Refusal, Register,
    RelPath, Related, Relates, Schema, Set, WriterId,
};

const NAME: Register<String> = Register::new("name");
const NOTE: Register<Option<String>> = Register::new("note");
const TAGS: Set<String> = Set::new("tags");
const SOURCES: Set<Source> = Set::new("sources");
const TEMPLATE: Register<EntityId> = Register::new("template");

/// A value of a relation that holds more than the entity it names.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Serialize, Deserialize)]
struct Source {
    to: EntityId,
    role: String,
}

impl Relates for Source {
    fn target(&self) -> EntityId {
        self.to
    }
}

fn schema() -> Schema {
    let keys = [
        NAME.key(),
        NOTE.key(),
        TAGS.key(),
        SOURCES.key(),
        TEMPLATE.key(),
    ];
    Schema::of(&keys).unwrap()
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
        bindings: Bindings::default().into(),
        listing: None,
        writers: Vec::new(),
        forks: Vec::new(),
        gaps: Vec::new(),
        items: Vec::new(),
    })
}

/// One writer's chain or snapshot, as a reader sees it.
enum Shown<'a> {
    Lines(&'a [Rc<Entry>]),
    Snapshot(Rc<Snapshot>),
}

/// What a reader derives from what each writer's directory shows.
fn derived(shown: Vec<(WriterId, Shown<'_>)>) -> Folded {
    let seen = shown
        .into_iter()
        .map(|(writer, shown)| {
            let seen = match shown {
                Shown::Lines(entries) => Seen {
                    segments: vec![SeenSegment {
                        entries,
                        sealed: false,
                    }],
                    snapshots: Vec::new(),
                },
                Shown::Snapshot(snapshot) => Seen {
                    segments: Vec::new(),
                    snapshots: vec![snapshot],
                },
            };
            (writer, seen)
        })
        .collect();
    derive(&seen).folded
}

/// `writer`'s chain `entries` folded into a snapshot that names each of them,
/// as another reader decodes it.
fn compacted(writer: WriterId, entries: &[Rc<Entry>]) -> Rc<Snapshot> {
    let mut state = Contribution::default();
    for entry in entries {
        state.fold(writer, entry);
    }
    let last = entries.last().expect("a chain");
    let snapshot = Snapshot {
        writer,
        label: String::new(),
        at: last.at(),
        genesis: entries[0].hash(),
        head: last.own(),
        certified: Certified {
            hashes: entries.iter().map(|entry| entry.own()).collect(),
            ..Certified::default()
        },
        state,
        unknown: BTreeMap::new(),
    };
    Rc::new(Snapshot::decode(&snapshot.encode()).unwrap())
}

/// One instance writing as `id`, with what it has seen merged in `folded`.
#[derive(Clone)]
struct Writer {
    id: WriterId,
    clock: Hlc,
    /// Every writer's chain it has seen, its own included.
    chains: BTreeMap<WriterId, Vec<Rc<Entry>>>,
    folded: Folded,
    own: Vec<Entry>,
    history: History,
}

impl Writer {
    fn new(id: u128) -> Self {
        Self {
            id: WriterId::from_u128(id),
            clock: Hlc::ZERO,
            chains: BTreeMap::new(),
            folded: Folded::default(),
            own: Vec::new(),
            history: History::default(),
        }
    }

    /// Appends `kind` to its chain, keeping an intent in its history with what
    /// it replaced, as `entering` says.
    fn log(&mut self, now: u64, kind: EntryKind, entering: Entering) -> Entry {
        self.clock = self.clock.tick(now).unwrap();
        let (prev, n) = self
            .own
            .last()
            .map_or((EntryHash::ZERO, 0), |last| (last.hash(), last.n() + 1));
        let before = match &kind {
            EntryKind::Intent(logged) => Before::of(&logged.ops, &self.folded, &[]),
            _ => Before::default(),
        };
        let logged = match &kind {
            EntryKind::Intent(logged) => Some(logged.clone()),
            _ => None,
        };
        let entry = Entry::encode(prev, n, self.clock, kind).unwrap();
        if let Some(logged) = logged {
            self.history.push(&entry, logged, entering, before);
        }
        self.own.push(entry.clone());
        self.receive(self.id, &entry);
        entry
    }

    fn intent(&mut self, now: u64, ops: Vec<Op>, displaced: Vec<Displaced>) -> Entry {
        let logged = Logged {
            label: "test".into(),
            ops,
            displaced,
            forks: Vec::new(),
            record: None,
        };
        self.log(now, EntryKind::Intent(logged), Entering::Step)
    }

    fn commit(&mut self, plan: Plan, now: u64) -> Result<Entry, Invalid> {
        let ops = ops(&plan, &self.folded, &schema())?;
        let entering = Entering::of(&plan);
        let logged = Logged {
            label: plan.label,
            ops,
            displaced: Vec::new(),
            forks: Vec::new(),
            record: None,
        };
        Ok(self.log(now, EntryKind::Intent(logged), entering))
    }

    fn receive(&mut self, writer: WriterId, entry: &Entry) {
        self.clock = self.clock.observe(entry.at());
        let chain = self.chains.entry(writer).or_default();
        if !chain.iter().any(|held| held.hash() == entry.hash()) {
            chain.push(Rc::new(entry.clone()));
        }
        let shown = self
            .chains
            .iter()
            .map(|(writer, chain)| (*writer, Shown::Lines(chain)));
        self.folded = derived(shown.collect());
    }

    fn sync(&mut self, from: &Writer) {
        for entry in &from.own {
            self.receive(from.id, entry);
        }
    }

    fn view(&self) -> View {
        view(self.folded.clone())
    }

    /// What it shows once every chain it has seen is compacted.
    fn compacted(&self) -> Folded {
        let shown = self
            .chains
            .iter()
            .map(|(writer, chain)| (*writer, Shown::Snapshot(compacted(*writer, chain))));
        derived(shown.collect())
    }

    fn undo(&mut self, now: u64) -> Result<Entry, Refusal> {
        let plan = self.history.plan_undo(self.id, &self.view())?;
        Ok(self.commit(plan, now).unwrap())
    }

    fn redo(&mut self, now: u64) -> Result<Entry, Refusal> {
        let plan = self.history.plan_redo(self.id, &self.view())?;
        Ok(self.commit(plan, now).unwrap())
    }
}

/// Two writers who have both seen `e` created by the first with `name` "first"
/// and tag "a".
fn pair(e: EntityId) -> (Writer, Writer) {
    let mut a = Writer::new(0xa);
    let mut b = Writer::new(0xb);
    let (create, _) = creating("Add", &[e]).create(|new| {
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
    let note = |writer: &Writer| view(writer.compacted()).entity(e).unwrap().get(NOTE);
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
                .register_writes(e, "name")
                .iter()
                .map(Write::named)
                .collect(),
            take: None,
        },
        Op::Add {
            entity: e,
            key: "tags".into(),
            value: number.clone(),
            replaces: Vec::new(),
            take: None,
        },
    ];
    a.intent(2, ops, Vec::new());
    for folded in [a.folded.clone(), a.compacted()] {
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
    let tag = a.folded.tags(e, "tags", &Raw::of(&"a").unwrap())[0].named();

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
    let mut expected = vec![left.own().of(a.id), right.own().of(b.id), tag];
    expected.sort();
    let mut observed: Vec<Ref> = observed.iter().map(|observed| observed.at).collect();
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

    let mut both = a.clone();
    both.sync(&b);
    let (a_chain, b_chain) = (&both.chains[&a.id], &both.chains[&b.id]);
    let cases = [
        (
            Shown::Snapshot(compacted(a.id, a_chain)),
            Shown::Lines(b_chain),
        ),
        (
            Shown::Lines(a_chain),
            Shown::Snapshot(compacted(b.id, b_chain)),
        ),
        (
            Shown::Snapshot(compacted(a.id, a_chain)),
            Shown::Snapshot(compacted(b.id, b_chain)),
        ),
    ];
    for (of_a, of_b) in cases {
        let reader = view(derived(vec![(a.id, of_a), (b.id, of_b)]));
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
    let EntryKind::Intent(logged) = undo.kind() else {
        panic!();
    };
    assert_eq!(logged.label, "Rename");
    let undone: Vec<_> = a
        .history
        .items()
        .iter()
        .map(|i| (i.label.as_str(), i.undone))
        .collect();
    assert_eq!(undone, [("Add", false), ("Rename", true)]);

    a.redo(4).unwrap();
    assert_eq!(name(&a, e), Field::Value("second".into()));
    assert!(!a.history.items()[1].undone);
    assert_eq!(a.history.items()[1].intent, rename.hash());
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
fn an_undo_let_go_of_after_its_refusal_leaves_the_intent_before_it_to_undo() {
    let e = entity(1);
    let (mut a, mut b) = pair(e);
    a.commit(plan(intent("").set(e, NAME, "mine".into())), 2)
        .unwrap();
    a.commit(plan(intent("").add(e, TAGS, "b".into())), 3)
        .unwrap();
    b.sync(&a);
    b.commit(plan(intent("").remove(e, TAGS, &"b".into())), 4)
        .unwrap();
    a.sync(&b);
    assert!(matches!(a.undo(5), Err(Refusal::ChangedSince { .. })));
    a.history.forget_undo();
    a.undo(6).unwrap();
    assert_eq!(name(&a, e), Field::Value("first".into()));
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
                take: None,
            }];
            replaces.push(a.intent(2, ops, Vec::new()).own().of(a.id));
        }
        let file = Op::File {
            entity: e,
            file: after,
            replaces,
            take: None,
        };
        let pin = Op::Pin {
            entity: entity(9),
            file: fact("moved.bin", 3),
            replaces: Vec::new(),
            take: None,
        };
        let changed = a.intent(3, vec![file, pin], trashed);
        let plan = a.history.plan_undo(a.id, &a.view()).unwrap();
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

        let kind = a.log(2, EntryKind::Unknown(unknown_kind.clone()), Entering::Step);
        let replaces = a
            .folded
            .register_writes(e, "name")
            .iter()
            .map(Write::named)
            .collect();
        let ops = vec![
            Op::Unknown(unknown_op.clone()),
            Op::Write {
                entity: e,
                key: "future".into(),
                value: Some(undeclared.clone()),
                replaces: Vec::new(),
                take: None,
            },
            Op::Write {
                entity: e,
                key: "name".into(),
                value: Some(unreadable.clone()),
                replaces,
                take: None,
            },
        ];
        let op = a.intent(3, ops, Vec::new());

        for (how, folded) in [("merged", a.folded.clone()), ("compacted", a.compacted())] {
            let unknown: Vec<_> = folded.unknown().cloned().collect();
            let mut expected = vec![
                (kind.hash(), Raw::new(&kind.json()).unwrap()),
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
    }
}

/// Members no build knows, added to JSON objects: each gets a fresh name and a
/// value from [`json`], kept as text until [`Novel::render`].
struct Novel<'a> {
    random: &'a mut SeededRandom,
    members: Vec<(String, String)>,
}

impl Novel<'_> {
    fn add(&mut self, object: &mut serde_json::Value) {
        let n = self.members.len();
        let name = format!("novel{n}");
        let text = json(self.random, 2);
        let slot = object.as_object_mut().expect("a record is an object");
        slot.insert(name.clone(), serde_json::Value::String(format!("@{n}@")));
        self.members.push((name, text));
    }

    fn add_each(&mut self, array: Option<&mut serde_json::Value>) {
        for item in array.and_then(|a| a.as_array_mut()).into_iter().flatten() {
            self.add(item);
        }
    }

    /// `value` as text, with each member's text in place.
    fn render(&self, value: &serde_json::Value) -> String {
        let mut text = value.to_string();
        for (n, (_, member)) in self.members.iter().enumerate() {
            text = text.replace(&format!("\"@{n}@\""), member);
        }
        text
    }

    /// Whether `text` holds every member byte for byte.
    fn all_in(&self, text: &str) -> Result<(), String> {
        let lost: Vec<_> = self
            .members
            .iter()
            .filter(|(name, member)| !text.contains(&format!("\"{name}\":{member}")))
            .collect();
        match lost.is_empty() {
            true => Ok(()),
            false => Err(format!("lost {lost:?} from {text}")),
        }
    }
}

/// Adds a novel member to the entry and to each op, file and displaced item in it.
fn novel_entry(novel: &mut Novel, entry: &mut serde_json::Value) {
    novel.add(entry);
    for op in entry
        .get_mut("ops")
        .and_then(|ops| ops.as_array_mut())
        .into_iter()
        .flatten()
    {
        novel.add(op);
        if let Some(file) = op.get_mut("file") {
            novel.add(file);
        }
    }
    novel.add_each(entry.get_mut("displaced"));
}

/// A history touching every part of the state: a genesis entry, then intents
/// with every op, a displaced item and a settlement, each line holding what
/// `edit` makes of the JSON this build writes.
fn every_part(mut edit: impl FnMut(&str) -> String) -> Vec<Entry> {
    use toshokan::log::{Genesis, Settle, Settlement};
    let w = WriterId::from_u128(0xa);
    let (e, gone) = (entity(1), entity(2));
    let fact = |at: &str| FileFact {
        path: RelPath::new(at).unwrap(),
        identity: Identity::from_u128(7),
        len: 3,
        modified: Some(4),
    };
    let raw = |text: &str| Raw::new(text).unwrap();
    let mut entries: Vec<Entry> = Vec::new();
    let mut log = |kind: EntryKind| -> Ref {
        let prev = entries.last().map_or(EntryHash::ZERO, Entry::hash);
        let n = entries.len() as u64;
        let at = Hlc {
            wall_ms: n + 1,
            counter: 0,
        };
        let canonical = Entry::encode(prev, n, at, kind).unwrap();
        let line = Line::seal(edit(&canonical.json())).unwrap();
        let entry = Entry::decode(line).unwrap();
        let named = entry.own().of(w);
        entries.push(entry);
        named
    };
    let intent = |ops: Vec<Op>, displaced: Vec<Displaced>| {
        EntryKind::Intent(Logged {
            label: "every part".into(),
            ops,
            displaced,
            forks: Vec::new(),
            record: None,
        })
    };
    log(EntryKind::Genesis(Genesis {
        writer: w,
        label: "a".into(),
    }));
    let first = log(intent(
        vec![
            Op::Create {
                entity: e,
                replaces: Vec::new(),
                take: None,
            },
            Op::Create {
                entity: gone,
                replaces: Vec::new(),
                take: None,
            },
            Op::Write {
                entity: e,
                key: "name".into(),
                value: Some(raw("\"x\"")),
                replaces: Vec::new(),
                take: None,
            },
            Op::Add {
                entity: e,
                key: "tags".into(),
                value: raw("\"a\""),
                replaces: Vec::new(),
                take: None,
            },
            Op::File {
                entity: e,
                file: Some(fact("a.bin")),
                replaces: Vec::new(),
                take: None,
            },
        ],
        vec![Displaced {
            item: Nonce::from_u128(5),
            from: RelPath::new("a.bin").unwrap(),
            identity: Identity::from_u128(6),
            len: 2,
        }],
    ));
    log(intent(
        vec![
            Op::Add {
                entity: e,
                key: "tags".into(),
                value: raw("\"b\""),
                replaces: Vec::new(),
                take: None,
            },
            Op::Remove {
                entity: e,
                key: "tags".into(),
                value: raw("\"a\""),
                tags: vec![first],
                take: None,
            },
            Op::Write {
                entity: e,
                key: "note".into(),
                value: None,
                replaces: Vec::new(),
                take: None,
            },
            Op::Pin {
                entity: e,
                file: fact("b.bin"),
                replaces: vec![first],
                take: None,
            },
            Op::Delete {
                entity: gone,
                replaces: vec![first],
                observed: Vec::new(),
                into: None,
                take: None,
            },
        ],
        Vec::new(),
    ));
    log(EntryKind::Settle(Settle {
        writer: WriterId::from_u128(0xb),
        record: Nonce::from_u128(9),
        outcome: Settlement::Dismissed,
    }));
    entries
}

/// What `entries`, the chain of writer `0xa`, merge to: as lines, or compacted.
fn folded(entries: &[Entry]) -> [Folded; 2] {
    let w = WriterId::from_u128(0xa);
    let chain: Vec<Rc<Entry>> = entries.iter().cloned().map(Rc::new).collect();
    [
        derived(vec![(w, Shown::Lines(&chain))]),
        derived(vec![(w, Shown::Snapshot(compacted(w, &chain)))]),
    ]
}

#[test]
fn members_no_build_knows_survive_merges_and_snapshots() {
    for seed in 0..100 {
        let mut random = SeededRandom::new(seed);
        let mut novel = Novel {
            random: &mut random,
            members: Vec::new(),
        };
        let entries = every_part(|json| {
            let mut entry = serde_json::from_str(json).unwrap();
            novel_entry(&mut novel, &mut entry);
            novel.render(&entry)
        });
        assert!(entries.iter().all(|entry| entry.unknown_members()));
        let whole: BTreeSet<(EntryHash, Raw)> = entries
            .iter()
            .map(|entry| (entry.hash(), Raw::new(&entry.json()).unwrap()))
            .collect();
        for (how, state) in ["merged", "compacted"].into_iter().zip(folded(&entries)) {
            let kept: BTreeSet<_> = state.extended().cloned().collect();
            assert_eq!(kept, whole, "seed {seed}, {how}");
        }
        let chain: Vec<Rc<Entry>> = entries.iter().cloned().map(Rc::new).collect();
        let snapshot = compacted(WriterId::from_u128(0xa), &chain).encode();
        novel
            .all_in(&String::from_utf8(snapshot).unwrap())
            .unwrap_or_else(|lost| panic!("seed {seed}: {lost}"));

        let canonical = every_part(str::to_owned);
        assert!(canonical.iter().all(|entry| !entry.unknown_members()));
        for state in folded(&canonical) {
            assert_eq!(state.extended().count(), 0, "seed {seed}");
        }
    }
}

fn source(to: EntityId) -> Source {
    Source {
        to,
        role: "take".into(),
    }
}

/// Every entity shown holding `sources`, each with what its sources resolve to.
fn sources(view: &View) -> Vec<(EntityId, Vec<Option<EntityId>>)> {
    let mut by_holder: BTreeMap<EntityId, Vec<Option<EntityId>>> = BTreeMap::new();
    for related in view.relations(SOURCES).related {
        by_holder.entry(related.from).or_default().push(related.to);
    }
    by_holder.into_iter().collect()
}

/// A writer that made the entities `made` and then `library`, whose sources
/// name each of `named`.
fn library_of(made: &[EntityId], library: EntityId, named: &[EntityId]) -> Writer {
    let mut a = Writer::new(0xa);
    for (at, made) in made.iter().enumerate() {
        let (create, _) = creating("Add", &[*made]).create(|new| {
            new.set(NAME, format!("made {at}"));
        });
        a.commit(plan(create), 1).unwrap();
    }
    let (create, _) = creating("Build", &[library]).create(|new| {
        for named in named {
            new.add(SOURCES, source(*named));
        }
    });
    a.commit(plan(create), 2).unwrap();
    a
}

#[test]
fn a_relation_reads_each_value_with_the_entity_it_names() {
    let [take, donor, library] = [entity(1), entity(2), entity(3)];
    let mut a = library_of(&[take, donor], library, &[take]);
    a.commit(plan(intent("").set(library, TEMPLATE, donor)), 3)
        .unwrap();
    for folded in [a.folded.clone(), a.compacted()] {
        let view = view(folded);
        let held = view.entity(library).unwrap();
        let named = Related {
            from: library,
            value: source(take),
            to: Some(take),
        };
        assert_eq!(
            held.relations(SOURCES).related,
            std::slice::from_ref(&named)
        );
        assert_eq!(view.relations(SOURCES).related, [named]);
        let template = Related {
            from: library,
            value: donor,
            to: Some(donor),
        };
        assert_eq!(view.relations(TEMPLATE).related, [template]);
        assert_eq!(view.referrers(SOURCES, take), [library]);
        assert_eq!(view.referrers(SOURCES, donor), []);
        assert_eq!(view.referrers(TEMPLATE, donor), [library]);
        assert_eq!(view.entity(take).unwrap().relations(SOURCES).related, []);
    }
}

#[test]
fn a_deleted_target_stays_named_and_resolves_again_once_it_is_brought_back() {
    let [take, library] = [entity(1), entity(2)];
    let mut a = library_of(&[take], library, &[take]);
    a.commit(plan(intent("Delete").delete(take)), 3).unwrap();
    let view = a.view();
    assert_eq!(view.resolve(take), None);
    assert_eq!(sources(&view), [(library, vec![None])]);
    let [related] = &view.relations(SOURCES).related[..] else {
        panic!("the value is kept");
    };
    assert_eq!(related.value, source(take));
    assert_eq!(view.referrers(SOURCES, take), [library], "by the id named");

    a.undo(4).unwrap();
    let view = a.view();
    assert_eq!(sources(&view), [(library, vec![Some(take)])]);
    assert_eq!(view.referrers(SOURCES, take), [library]);
}

#[test]
fn a_target_whose_writer_has_not_arrived_resolves_once_it_does() {
    let [take, library] = [entity(1), entity(2)];
    let a = library_of(&[take], entity(9), &[]);
    let mut b = Writer::new(0xb);
    b.sync(&a);
    let (create, _) = creating("Build", &[library]).create(|new| {
        new.add(SOURCES, source(take));
    });
    b.commit(plan(create), 3).unwrap();

    let mut c = Writer::new(0xc);
    c.sync(&b);
    assert_eq!(sources(&c.view()), [(library, vec![None])]);
    c.sync(&a);
    assert_eq!(sources(&c.view()), [(library, vec![Some(take)])]);
}

/// Writers `a` and `b` who have both seen `low` and `high`, each made by one of
/// them as two writers adopt one file, and `library`, which names `high`.
fn named_twice(low: EntityId, high: EntityId, library: EntityId) -> (Writer, Writer) {
    let mut a = library_of(&[high], library, &[high]);
    let mut b = Writer::new(0xb);
    let (create, _) = creating("Tag", &[low]).create(|new| {
        new.add(TAGS, "b".into());
    });
    b.commit(plan(create), 1).unwrap();
    a.sync(&b);
    b.sync(&a);
    (a, b)
}

#[test]
fn what_names_a_joined_entity_names_the_one_it_was_joined_into() {
    let [low, high, library] = [entity(1), entity(2), entity(3)];
    let (mut a, mut b) = named_twice(low, high, library);
    a.commit(plan(intent("Join").join(high, low)), 3).unwrap();
    b.sync(&a);
    for folded in [a.folded.clone(), b.folded.clone(), b.compacted()] {
        let view = view(folded);
        assert!(view.entity(high).is_none());
        assert_eq!(view.resolve(high), Some(low));
        assert_eq!(view.resolve(low), Some(low));
        assert_eq!(sources(&view), [(library, vec![Some(low)])]);
        assert_eq!(view.referrers(SOURCES, low), [library]);
        assert_eq!(view.referrers(SOURCES, high), [library]);
        assert_eq!(view.conflicts(), []);
    }
}

#[test]
fn a_relation_made_without_seeing_the_join_names_the_survivor_too() {
    let [low, high, library, later] = [entity(1), entity(2), entity(3), entity(4)];
    let (mut a, mut b) = named_twice(low, high, library);
    a.commit(plan(intent("Join").join(high, low)), 3).unwrap();
    let (create, _) = creating("Build", &[later]).create(|new| {
        new.add(SOURCES, source(high));
    });
    b.commit(plan(create), 3).unwrap();
    a.sync(&b);
    b.sync(&a);
    for writer in [&a, &b] {
        let view = writer.view();
        assert_eq!(view.conflicts(), []);
        assert_eq!(
            sources(&view),
            [(library, vec![Some(low)]), (later, vec![Some(low)])]
        );
        assert_eq!(view.referrers(SOURCES, low), [library, later]);
    }
}

#[test]
fn writers_who_each_join_one_entity_agree_on_what_it_names() {
    let [low, high, library] = [entity(1), entity(2), entity(3)];
    let (mut a, mut b) = named_twice(low, high, library);
    for writer in [&mut a, &mut b] {
        writer
            .commit(plan(intent("Join").join(high, low)), 3)
            .unwrap();
    }
    a.sync(&b);
    b.sync(&a);
    for writer in [&a, &b] {
        let view = writer.view();
        assert_eq!(view.conflicts(), []);
        assert_eq!(view.resolve(high), Some(low));
        assert_eq!(sources(&view), [(library, vec![Some(low)])]);
    }
}

#[test]
fn an_entity_joined_into_one_that_is_then_joined_resolves_to_the_last() {
    let [low, middle, high, library] = [entity(1), entity(2), entity(3), entity(4)];
    let mut a = library_of(&[low, middle, high], library, &[high, middle]);
    a.commit(plan(intent("Join").join(high, middle)), 3)
        .unwrap();
    a.commit(plan(intent("Join").join(middle, low)), 4).unwrap();
    let view = a.view();
    assert_eq!(view.resolve(high), Some(low));
    assert_eq!(view.resolve(middle), Some(low));
    assert_eq!(sources(&view), [(library, vec![Some(low), Some(low)])]);
    assert_eq!(view.referrers(SOURCES, high), [library], "once");
}

#[test]
fn joins_of_one_intent_that_follow_on_give_the_last_entity_everything() {
    let [low, middle, high, library] = [entity(1), entity(2), entity(3), entity(4)];
    for joins in [
        [(high, middle), (middle, low)],
        [(middle, low), (high, middle)],
    ] {
        let mut a = library_of(&[low, middle, high], library, &[high, middle]);
        a.commit(plan(intent("").add(high, TAGS, "high".into())), 3)
            .unwrap();
        let joining = joins
            .iter()
            .fold(intent("Join"), |built, (one, into)| built.join(*one, *into));
        a.commit(plan(joining), 4).unwrap();
        let view = a.view();
        assert_eq!(view.entities().len(), 2, "{joins:?}");
        assert_eq!(tags(&a, low), ["high"], "{joins:?}");
        assert_eq!(
            (view.resolve(high), view.resolve(middle)),
            (Some(low), Some(low)),
            "{joins:?}"
        );
        assert_eq!(sources(&view), [(library, vec![Some(low), Some(low)])]);
    }
}

#[test]
fn joins_of_one_intent_that_lead_back_are_refused() {
    let [one, other] = [entity(1), entity(2)];
    let a = library_of(&[one, other], entity(3), &[]);
    let around = plan(intent("Join").join(one, other).join(other, one));
    assert_eq!(
        ops(&around, &a.folded, &schema()),
        Err(Invalid::JoinedIntoItself(one))
    );
}

#[test]
fn deleting_the_entity_joined_into_unresolves_what_named_either_until_undo() {
    let [low, high, library] = [entity(1), entity(2), entity(3)];
    let (mut a, _) = named_twice(low, high, library);
    a.commit(plan(intent("Join").join(high, low)), 3).unwrap();
    let before = a.view();
    a.commit(plan(intent("Delete").delete(low)), 4).unwrap();
    let view = a.view();
    assert_eq!((view.resolve(high), view.resolve(low)), (None, None));
    assert_eq!(sources(&view), [(library, vec![None])]);
    let changed = view.changed_since(&before).entities;
    assert_eq!(changed, BTreeSet::from([low]), "not the entity naming it");
    assert_eq!(before.referrers(SOURCES, low), [library], "found before");
    assert_eq!(view.referrers(SOURCES, low), [], "and not after");

    a.undo(5).unwrap();
    let view = a.view();
    assert_eq!(view.resolve(high), Some(low));
    assert_eq!(sources(&view), [(library, vec![Some(low)])]);
}

#[test]
fn undoing_an_intent_that_also_joined_takes_back_the_join() {
    let [low, high, library] = [entity(1), entity(2), entity(3)];
    let (mut a, _) = named_twice(low, high, library);
    let joining = intent("Tidy").join(high, low).add(low, TAGS, "kept".into());
    a.commit(plan(joining), 3).unwrap();
    assert_eq!(a.view().resolve(high), Some(low));

    a.undo(4).unwrap();
    let view = a.view();
    assert_eq!(view.resolve(high), Some(high), "itself again");
    assert_eq!(sources(&view), [(library, vec![Some(high)])]);

    a.redo(5).unwrap();
    let view = a.view();
    assert_eq!(view.resolve(high), Some(low), "joined again");
    assert_eq!(sources(&view), [(library, vec![Some(low)])]);
}

#[test]
fn an_entity_brought_back_is_itself_until_it_is_joined_again() {
    let [low, high, library] = [entity(1), entity(2), entity(3)];
    let (mut a, _) = named_twice(low, high, library);
    a.commit(plan(intent("Join").join(high, low)), 3).unwrap();
    a.commit(plan(intent("Keep").revive(high)), 4).unwrap();
    let view = a.view();
    assert_eq!(view.resolve(high), Some(high));
    assert_eq!(sources(&view), [(library, vec![Some(high)])]);

    a.commit(plan(intent("Delete").delete(high)), 5).unwrap();
    let view = a.view();
    assert_eq!(
        view.resolve(high),
        None,
        "the join it was brought back from"
    );
    assert_eq!(sources(&view), [(library, vec![None])]);

    a.commit(plan(intent("Keep").revive(high)), 6).unwrap();
    a.commit(plan(intent("Join").join(high, library)), 7)
        .unwrap();
    assert_eq!(a.view().resolve(high), Some(library));
}

#[test]
fn undoing_the_return_of_a_joined_entity_joins_it_again() {
    let [low, high, library] = [entity(1), entity(2), entity(3)];
    let (mut a, _) = named_twice(low, high, library);
    a.commit(plan(intent("Join").join(high, low)), 3).unwrap();
    a.commit(plan(intent("Keep").revive(high)), 4).unwrap();
    a.undo(5).unwrap();
    for folded in [a.folded.clone(), a.compacted()] {
        let view = view(folded);
        assert_eq!(view.resolve(high), Some(low));
        assert_eq!(sources(&view), [(library, vec![Some(low)])]);
    }
}

#[test]
fn writers_who_join_one_entity_into_different_ones_name_the_lowest() {
    let [first, second, gone, library] = [entity(1), entity(2), entity(7), entity(3)];
    let mut a = library_of(&[first, second, gone], library, &[gone]);
    let mut b = Writer::new(0xb);
    b.sync(&a);
    a.commit(plan(intent("Join").join(gone, second)), 3)
        .unwrap();
    b.commit(plan(intent("Join").join(gone, first)), 3).unwrap();
    a.sync(&b);
    b.sync(&a);
    for folded in [a.folded.clone(), b.folded.clone(), a.compacted()] {
        let view = view(folded);
        assert_eq!(view.resolve(gone), Some(first));
        assert_eq!(sources(&view), [(library, vec![Some(first)])]);
    }

    a.commit(plan(intent("Delete").delete(first)), 4).unwrap();
    assert_eq!(a.view().resolve(gone), Some(second), "the one still shown");
}

/// A writer that made `made`, each with no field, and then `library`, whose
/// sources name each of `named`.
fn bare(made: &[EntityId], library: EntityId, named: &[EntityId]) -> Writer {
    let mut a = Writer::new(0xa);
    let create = made
        .iter()
        .fold(creating("Add", made), |built, _| built.create(|_| {}).0);
    a.commit(plan(create), 1).unwrap();
    let (create, _) = creating("Build", &[library]).create(|new| {
        for named in named {
            new.add(SOURCES, source(*named));
        }
    });
    a.commit(plan(create), 2).unwrap();
    a
}

#[test]
fn joins_two_writers_made_apart_are_followed_through_both() {
    let [low, middle, high, library] = [entity(1), entity(2), entity(3), entity(4)];
    let mut a = bare(&[low, middle, high], library, &[high]);
    let mut b = Writer::new(0xb);
    b.sync(&a);
    a.commit(plan(intent("Join").join(high, middle)), 3)
        .unwrap();
    b.commit(plan(intent("Join").join(middle, low)), 3).unwrap();
    a.sync(&b);
    b.sync(&a);
    for folded in [a.folded.clone(), b.folded.clone(), b.compacted()] {
        let view = view(folded);
        assert_eq!(view.conflicts(), []);
        assert_eq!(
            (view.resolve(high), view.resolve(middle)),
            (Some(low), Some(low))
        );
        assert_eq!(sources(&view), [(library, vec![Some(low)])]);
        assert_eq!(view.referrers(SOURCES, low), [library]);
    }
}

#[test]
fn a_join_into_an_entity_another_writer_joined_away_keeps_it_in_conflict() {
    let [low, middle, high, library] = [entity(1), entity(2), entity(3), entity(4)];
    let mut a = library_of(&[low, middle, high], library, &[high]);
    let mut b = Writer::new(0xb);
    b.sync(&a);
    a.commit(plan(intent("").add(high, TAGS, "high".into())), 3)
        .unwrap();
    a.commit(plan(intent("Join").join(high, middle)), 4)
        .unwrap();
    b.commit(plan(intent("Join").join(middle, low)), 4).unwrap();
    a.sync(&b);
    let view = a.view();
    assert_eq!(view.conflicts(), [Conflicted::Existence { entity: middle }]);
    assert_eq!(
        tags(&a, middle),
        ["high"],
        "what the other join did not see"
    );
    assert_eq!(view.resolve(high), Some(middle));

    a.commit(plan(intent("Join").join(middle, low)), 5).unwrap();
    let view = a.view();
    assert_eq!(view.conflicts(), []);
    assert_eq!(tags(&a, low), ["high"]);
    assert_eq!(view.resolve(high), Some(low));
}

#[test]
fn a_join_into_an_entity_deleted_meanwhile_leaves_it_deleted() {
    let [low, high, library] = [entity(1), entity(2), entity(3)];
    let mut a = bare(&[low, high], library, &[high]);
    let mut b = Writer::new(0xb);
    b.sync(&a);
    a.commit(plan(intent("Delete").delete(low)), 3).unwrap();
    b.commit(plan(intent("Join").join(high, low)), 3).unwrap();
    a.sync(&b);
    b.sync(&a);
    for writer in [&a, &b] {
        let view = writer.view();
        assert_eq!(view.conflicts(), []);
        assert_eq!(view.entities().len(), 1, "the library alone");
        assert_eq!((view.resolve(high), view.resolve(low)), (None, None));
        assert_eq!(sources(&view), [(library, vec![None])]);
    }
}

#[test]
fn a_delete_op_with_no_entity_joined_into_reads_as_a_plain_delete() {
    let [low, high, library] = [entity(1), entity(2), entity(3)];
    let (mut a, _) = named_twice(low, high, library);
    let joined = a.commit(plan(intent("Join").join(high, low)), 3).unwrap();
    let json = joined.json();
    let into = format!(r#","into":"{low}""#);
    assert_eq!(json.matches(&into).count(), 1, "{json}");

    let deleted = a.commit(plan(intent("Delete").delete(library)), 4).unwrap();
    assert!(!deleted.json().contains("into"), "{}", deleted.json());
}

#[test]
fn a_value_that_does_not_decode_names_nothing_and_is_listed() {
    let [take, library] = [entity(1), entity(2)];
    let mut a = library_of(&[take], library, &[take]);
    let newer = Raw::new(r#"{"into":"x"}"#).unwrap();
    let add = Op::Add {
        entity: library,
        key: "sources".into(),
        value: newer.clone(),
        replaces: Vec::new(),
        take: None,
    };
    a.intent(3, vec![add], Vec::new());
    let view = a.view();
    let relations = view.relations(SOURCES);
    assert_eq!(relations.unreadable, [(library, newer)]);
    assert_eq!(relations.related.len(), 1);
    assert_eq!(view.referrers(SOURCES, take), [library]);
}

#[test]
fn each_side_of_a_conflicted_relation_is_read() {
    let [one, two, library] = [entity(1), entity(2), entity(3)];
    let mut a = library_of(&[one, two], library, &[]);
    let mut b = Writer::new(0xb);
    b.sync(&a);
    a.commit(plan(intent("").set(library, TEMPLATE, one)), 3)
        .unwrap();
    b.commit(plan(intent("").set(library, TEMPLATE, two)), 3)
        .unwrap();
    a.sync(&b);
    let view = a.view();
    let named: Vec<Option<EntityId>> = view
        .relations(TEMPLATE)
        .related
        .into_iter()
        .map(|r| r.to)
        .collect();
    assert_eq!(named, [Some(one), Some(two)]);
    assert_eq!(view.referrers(TEMPLATE, one), [library]);
    assert_eq!(view.referrers(TEMPLATE, two), [library]);
}

/// Seeded walks of three writers that make entities, name them, join, delete,
/// bring back and sync at random. Once each holds every entry, all resolve each id
/// to the same entity, which they show, and agree on who names it, as does a
/// reader of their snapshots.
#[test]
fn writers_that_hold_the_same_entries_resolve_every_id_alike() {
    for seed in 0..200 {
        let mut random = SeededRandom::new(seed);
        let mut pick = |below: usize| (random.next_u128() % below as u128) as usize;
        let mut writers = [Writer::new(0xa), Writer::new(0xb), Writer::new(0xc)];
        let mut made: Vec<EntityId> = Vec::new();
        for step in 0..40u64 {
            let at = pick(3);
            let shown = writers[at].folded.entities();
            let now = step + 1;
            let any = |pick: &mut dyn FnMut(usize) -> usize, of: &[EntityId]| {
                (!of.is_empty()).then(|| of[pick(of.len())])
            };
            let built = match pick(7) {
                0 | 1 => {
                    let id = entity(made.len() as u128 + 1);
                    let named = any(&mut pick, &made);
                    made.push(id);
                    let (create, _) = creating("Add", &[id]).create(|new| {
                        new.set(NAME, "made".into());
                        if let Some(named) = named {
                            new.add(SOURCES, source(named));
                        }
                    });
                    Some(create)
                }
                2 => {
                    let pair = (any(&mut pick, &shown), any(&mut pick, &shown));
                    match pair {
                        (Some(one), Some(into)) if one != into => {
                            Some(intent("Join").join(one, into))
                        }
                        _ => None,
                    }
                }
                3 => any(&mut pick, &shown).map(|gone| intent("Delete").delete(gone)),
                4 => any(&mut pick, &made).map(|back| intent("Keep").revive(back)),
                5 => {
                    let pair = (any(&mut pick, &shown), any(&mut pick, &made));
                    match pair {
                        (Some(holder), Some(named)) => {
                            Some(intent("Name").add(holder, SOURCES, source(named)))
                        }
                        _ => None,
                    }
                }
                _ => {
                    let from = writers[(at + 1 + pick(2)) % 3].clone();
                    writers[at].sync(&from);
                    None
                }
            };
            if let Some(built) = built {
                // A change the writer's state refuses, such as a revive of an
                // entity it has not seen, is no step of the walk.
                let _ = writers[at].commit(plan(built), now);
            }
        }
        let held = writers.clone();
        for writer in &mut writers {
            for from in &held {
                writer.sync(from);
            }
        }
        let compacted = view(writers[0].compacted());
        let views: Vec<View> = writers
            .iter()
            .map(Writer::view)
            .chain([compacted])
            .collect();
        let read = |view: &View| {
            let resolved: Vec<Option<EntityId>> = made.iter().map(|id| view.resolve(*id)).collect();
            for to in resolved.iter().flatten() {
                assert!(view.entity(*to).is_some(), "seed {seed}: {to} is not shown");
                assert_eq!(
                    view.resolve(*to),
                    Some(*to),
                    "seed {seed}: {to} resolves on"
                );
            }
            let naming: Vec<Vec<EntityId>> =
                made.iter().map(|id| view.referrers(SOURCES, *id)).collect();
            for related in view.relations(SOURCES).related {
                let named = view.referrers(SOURCES, related.value.to);
                assert!(named.contains(&related.from), "seed {seed}: {related:?}");
            }
            (resolved, naming, sources(view))
        };
        let first = read(&views[0]);
        for view in &views[1..] {
            assert_eq!(read(view), first, "seed {seed}");
        }
    }
}
