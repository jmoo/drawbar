//! Facts: how entries merge, what a view shows, how intents become ops, and undo.

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

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
