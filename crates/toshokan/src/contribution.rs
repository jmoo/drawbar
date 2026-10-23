//! What one writer contributes to the merged state: its latest own write to each
//! field, live or not, and what its entries say of other writers' writes. A
//! snapshot holds a contribution; a reader folds the entries placed after it in
//! the same way.
//!
//! A field is one register, one member of a set, an entity's existence or its
//! file. A contribution keeps, per field, the latest own write with the number of
//! its first; per field and other writer, the latest reference at or below the
//! fork point of that writer its entry knew of and the latest above it; per own
//! field write, the first own delete above it; per field and other writer, the
//! latest take of a run; and its dismissals, trash, settlements and the entries
//! and ops this build does not know.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::Arc;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::ids::{EntityId, EntryHash, Hlc, Identity, Nonce, WriterId};
use crate::log::{
    Displaced, Entry, EntryKind, FileFact, Forked, Observed, Op, Own, Ref, Run, Target,
};
use crate::pack::{bad_variant, In, Pack, Unpack, Unpacked};
use crate::path::RelPath;
use crate::schema::Raw;

/// A key's name.
pub type Name = Arc<str>;

/// The most names [`name`] shares; past it, a new name is held by itself.
const NAMES: usize = 1 << 12;

/// `key` as a name, shared with every other name of the same text this thread
/// made, so a key many fields hold is held once.
pub fn name(key: &str) -> Name {
    thread_local! {
        static SHARED: RefCell<HashSet<Name>> = RefCell::default();
    }
    SHARED.with_borrow_mut(|shared| {
        if let Some(name) = shared.get(key) {
            return Arc::clone(name);
        }
        let name: Name = key.into();
        if shared.len() < NAMES {
            shared.insert(Arc::clone(&name));
        }
        name
    })
}

/// One field of one entity: what one write sets.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum FieldKey {
    Exist(EntityId),
    Register(EntityId, Name),
    /// A member of a set, by its value.
    Member(EntityId, Name, Raw),
    File(EntityId),
}

impl FieldKey {
    pub fn entity(&self) -> EntityId {
        match self {
            Self::Exist(entity)
            | Self::Register(entity, _)
            | Self::Member(entity, _, _)
            | Self::File(entity) => *entity,
        }
    }

    pub fn is_exist(&self) -> bool {
        matches!(self, Self::Exist(_))
    }

    /// The field of entity `entity` a delete observed a write of.
    pub fn observed(entity: EntityId, target: &Target) -> Self {
        match target {
            Target::Register(key) => Self::Register(entity, name(key)),
            Target::Member(key, value) => Self::Member(entity, name(key), value.clone()),
            Target::File => Self::File(entity),
        }
    }

    /// The target a delete names a write of this field by.
    pub fn target(&self) -> Option<Target> {
        match self {
            Self::Exist(_) => None,
            Self::Register(_, key) => Some(Target::Register(key.to_string())),
            Self::Member(_, key, value) => Some(Target::Member(key.to_string(), value.clone())),
            Self::File(_) => Some(Target::File),
        }
    }
}

/// What a write sets its field to.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Value {
    /// Existence: created, or deleted.
    Exist(bool),
    /// A register's value; `None` clears.
    Register(Option<Raw>),
    /// A set member: added, or removed.
    Member(bool),
    /// An entity's file; `pin` for a binding a scan derived.
    File { fact: Option<FileFact>, pin: bool },
}

/// One write: of which field, by which entry, and what.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Write {
    pub field: FieldKey,
    pub value: Value,
}

/// A writer's own write to a field, as its contribution holds it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Held {
    pub at: Own,
    pub stamp: Hlc,
    pub value: Value,
    /// The number of the writer's first own write to the field.
    pub first: u64,
}

/// A reference a contribution keeps: the own entry that made it, and its target.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Reference {
    pub own: Own,
    pub target: Ref,
}

/// A take a contribution keeps: the own entry that made it, and the first number
/// of the run it took, below which it settles the field for the run's writer.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Taken {
    pub own: Own,
    pub below: u64,
}

/// An item in a writer's trash: what displaced it, from where, and what it was.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Trashed {
    pub entry: EntryHash,
    pub at: Hlc,
    pub from: RelPath,
    pub identity: Identity,
    pub len: u64,
}

#[derive(Clone, PartialEq, Debug, Default)]
pub struct Contribution {
    pub own: BTreeMap<FieldKey, Held>,
    /// By field, other writer, and whether the target lies above the fork point
    /// of its writer that the own entry knew of.
    pub refs: BTreeMap<(FieldKey, WriterId, bool), Reference>,
    /// Per own field write, the first own delete of its entity in its entry or
    /// after.
    pub killed: BTreeMap<FieldKey, Own>,
    pub takes: BTreeMap<(FieldKey, WriterId), Taken>,
    /// The runs the writer dismissed, each with the own entry that did.
    pub dismissals: BTreeSet<(Own, Run)>,
    pub trash: BTreeMap<Nonce, Trashed>,
    pub settled: BTreeSet<(WriterId, Nonce)>,
    /// Own entries of a kind this build does not know, and ops it does not know,
    /// each with its entry.
    pub unknown: BTreeSet<(EntryHash, Raw)>,
    /// Own entries of a known kind holding members this build does not know,
    /// whole.
    pub extended: BTreeSet<(EntryHash, Raw)>,
    /// Members of a snapshot's `state` this build does not know, kept.
    pub later: BTreeMap<String, Raw>,
}

/// The writes, references and takes one op makes, with what else it says.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Said {
    pub writes: Vec<Write>,
    /// Each target with the field it is a reference on.
    pub refs: Vec<(FieldKey, Ref)>,
    pub takes: Vec<(FieldKey, Run)>,
    pub dismissals: Vec<Run>,
    /// Whether the op is a delete of this entity, which kills its writer's own
    /// writes of the entity's fields in its entry and before.
    pub deletes: Option<EntityId>,
}

/// What `op` says.
pub fn said(op: &Op) -> Said {
    let mut said = Said::default();
    let write =
        |said: &mut Said, field: FieldKey, value: Value, refs: &[Ref], take: Option<&Run>| {
            said.refs
                .extend(refs.iter().map(|target| (field.clone(), *target)));
            if let Some(run) = take {
                said.takes.push((field.clone(), *run));
            }
            said.writes.push(Write { field, value });
        };
    match op {
        Op::Create {
            entity,
            replaces,
            take,
        } => write(
            &mut said,
            FieldKey::Exist(*entity),
            Value::Exist(true),
            replaces,
            take.as_ref(),
        ),
        Op::Delete {
            entity,
            replaces,
            observed,
            take,
        } => {
            write(
                &mut said,
                FieldKey::Exist(*entity),
                Value::Exist(false),
                replaces,
                take.as_ref(),
            );
            for Observed { target, at } in observed {
                said.refs.push((FieldKey::observed(*entity, target), *at));
            }
            said.deletes = Some(*entity);
        }
        Op::Write {
            entity,
            key,
            value,
            replaces,
            take,
        } => write(
            &mut said,
            FieldKey::Register(*entity, name(key)),
            Value::Register(value.clone()),
            replaces,
            take.as_ref(),
        ),
        Op::Add {
            entity,
            key,
            value,
            replaces,
            take,
        } => write(
            &mut said,
            FieldKey::Member(*entity, name(key), value.clone()),
            Value::Member(true),
            replaces,
            take.as_ref(),
        ),
        Op::Remove {
            entity,
            key,
            value,
            tags,
            take,
        } => write(
            &mut said,
            FieldKey::Member(*entity, name(key), value.clone()),
            Value::Member(false),
            tags,
            take.as_ref(),
        ),
        Op::File {
            entity,
            file,
            replaces,
            take,
        } => write(
            &mut said,
            FieldKey::File(*entity),
            Value::File {
                fact: file.clone(),
                pin: false,
            },
            replaces,
            take.as_ref(),
        ),
        Op::Pin {
            entity,
            file,
            replaces,
            take,
        } => write(
            &mut said,
            FieldKey::File(*entity),
            Value::File {
                fact: Some(file.clone()),
                pin: true,
            },
            replaces,
            take.as_ref(),
        ),
        Op::Dismiss { runs } => said.dismissals.extend(runs.iter().copied()),
        Op::Unknown(_) => {}
    }
    said
}

/// The ops of `kind`, for the kinds that have them.
pub fn ops(kind: &EntryKind) -> &[Op] {
    match kind {
        EntryKind::Intent(logged) => &logged.ops,
        EntryKind::More(more) => &more.ops,
        EntryKind::Bind(bound) => &bound.ops,
        _ => &[],
    }
}

/// The entities whose fields an entry of `kind` kills as a delete does, beyond
/// those its ops delete: a continuation's, for its intent's deletes.
pub fn deletes(kind: &EntryKind) -> &[EntityId] {
    match kind {
        EntryKind::More(more) => &more.deletes,
        _ => &[],
    }
}

/// The bytes an entry of `kind` says its intent moved into its writer's trash.
pub fn displaced(kind: &EntryKind) -> &[Displaced] {
    match kind {
        EntryKind::Intent(logged) => &logged.displaced,
        EntryKind::More(more) => &more.displaced,
        _ => &[],
    }
}

/// The fork points an entry of `kind` says its writer knew of.
pub fn forks(kind: &EntryKind) -> &[Forked] {
    match kind {
        EntryKind::Intent(logged) => &logged.forks,
        EntryKind::More(more) => &more.forks,
        _ => &[],
    }
}

/// Of two references, the one a contribution keeps: the later target, then the
/// later own entry.
fn later(slot: &mut Option<Reference>, reference: Reference) {
    if slot.is_none_or(|kept| (reference.target.n, reference.own.n) > (kept.target.n, kept.own.n)) {
        *slot = Some(reference);
    }
}

impl Contribution {
    /// Folds `writer`'s own `entry`, the successor of every entry folded before.
    pub fn fold(&mut self, writer: WriterId, entry: &Entry) {
        let own = entry.own();
        let kind = entry.kind();
        if entry.unknown_members() {
            if let Ok(raw) = Raw::new(&entry.json()) {
                self.extended.insert((entry.hash(), raw));
            }
        }
        match &kind {
            EntryKind::Genesis(_) => {}
            EntryKind::Settle(settle) => {
                self.settled.insert((settle.writer, settle.record));
            }
            EntryKind::Unknown(raw) => {
                self.unknown.insert((entry.hash(), raw.clone()));
            }
            EntryKind::Intent(_) | EntryKind::More(_) | EntryKind::Bind(_) => {
                let known = forks(&kind);
                let mut deleted = Vec::new();
                for op in ops(&kind) {
                    if let Op::Unknown(raw) = op {
                        self.unknown.insert((entry.hash(), raw.clone()));
                        continue;
                    }
                    deleted.extend(self.fold_op(writer, own, entry.at(), known, op));
                }
                deleted.extend(deletes(&kind));
                for entity in deleted {
                    for (field, held) in &self.own {
                        if field.entity() == entity && !field.is_exist() && held.at.n <= own.n {
                            self.killed.entry(field.clone()).or_insert(own);
                        }
                    }
                }
                for Displaced {
                    item,
                    from,
                    identity,
                    len,
                } in displaced(&kind)
                {
                    self.trash.insert(
                        *item,
                        Trashed {
                            entry: entry.hash(),
                            at: entry.at(),
                            from: from.clone(),
                            identity: *identity,
                            len: *len,
                        },
                    );
                }
            }
        }
    }

    /// Folds `op` but for a delete's kills; returns the entity it deletes.
    fn fold_op(
        &mut self,
        writer: WriterId,
        own: Own,
        stamp: Hlc,
        known: &[Forked],
        op: &Op,
    ) -> Option<EntityId> {
        let said = said(op);
        for run in &said.dismissals {
            self.dismissals.insert((own, *run));
        }
        for Write { field, value } in said.writes {
            let first = self.own.get(&field).map_or(own.n, |held| held.first);
            self.killed.remove(&field);
            self.own.insert(
                field,
                Held {
                    at: own,
                    stamp,
                    value,
                    first,
                },
            );
        }
        for (field, target) in said.refs {
            if target.writer == writer {
                continue;
            }
            let above = known
                .iter()
                .any(|fork| fork.writer == target.writer && target.n > fork.at);
            let key = (field, target.writer, above);
            let mut slot = self.refs.get(&key).copied();
            later(&mut slot, Reference { own, target });
            self.refs.insert(key, slot.expect("set above"));
        }
        for (field, run) in said.takes {
            if run.writer != writer {
                self.takes.insert(
                    (field, run.writer),
                    Taken {
                        own,
                        below: run.from,
                    },
                );
            }
        }
        said.deletes
    }

    /// The fields of `entity` the contribution holds anything of.
    pub fn touches(&self) -> BTreeSet<EntityId> {
        let own = self.own.keys().map(FieldKey::entity);
        let refs = self.refs.keys().map(|(field, _, _)| field.entity());
        let killed = self.killed.keys().map(FieldKey::entity);
        let takes = self.takes.keys().map(|(field, _)| field.entity());
        own.chain(refs).chain(killed).chain(takes).collect()
    }
}

// The JSON of a contribution, the `state` of a snapshot.

/// A field of an entity as a contribution's records name it, in members of
/// their own: no member for its existence, `key` for a register, `key` and
/// `value` for a set's member, `file` for its file.
///
/// ⚠️ Written into each record by hand, not through `#[serde(flatten)]`, which
/// buffers the [`Raw`] value of a member.
#[derive(Default)]
struct FieldOut {
    key: Option<String>,
    value: Option<Raw>,
    file: bool,
}

fn present<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Raw>, D::Error> {
    Raw::deserialize(deserializer).map(Some)
}

impl FieldOut {
    fn of(field: &FieldKey) -> Self {
        match field {
            FieldKey::Exist(_) => Self::default(),
            FieldKey::Register(_, key) => Self {
                key: Some(key.to_string()),
                ..Self::default()
            },
            FieldKey::Member(_, key, value) => Self {
                key: Some(key.to_string()),
                value: Some(value.clone()),
                file: false,
            },
            FieldKey::File(_) => Self {
                file: true,
                ..Self::default()
            },
        }
    }

    fn read(key: Option<String>, value: Option<Raw>, file: bool) -> Self {
        Self { key, value, file }
    }

    fn field(self, entity: EntityId) -> Option<FieldKey> {
        Some(match (self.key, self.value, self.file) {
            (None, None, false) => FieldKey::Exist(entity),
            (Some(key), None, false) => FieldKey::Register(entity, name(&key)),
            (Some(key), Some(value), false) => FieldKey::Member(entity, name(&key), value),
            (None, None, true) => FieldKey::File(entity),
            _ => return None,
        })
    }
}

/// A held write: its field, its entry and stamp, the number of the first own
/// write to the field, and its value in the field's own terms.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HeldOut {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    key: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    value: Option<Raw>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    file: bool,
    at: Own,
    stamp: Hlc,
    first: u64,
    /// A register's value, or a file write's fact.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    set: Option<Raw>,
    /// A delete, a set's remove, or a file write that says the entity has none.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    off: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pin: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReferenceOut {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    key: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    value: Option<Raw>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    file: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    above: bool,
    own: Own,
    target: Ref,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct KilledOut {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    key: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    value: Option<Raw>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    file: bool,
    own: Own,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TakenOut {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    key: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    value: Option<Raw>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    file: bool,
    writer: WriterId,
    own: Own,
    below: u64,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct EntityOut {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    writes: Vec<HeldOut>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    refs: Vec<ReferenceOut>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    killed: Vec<KilledOut>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    takes: Vec<TakenOut>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DismissalOut {
    own: Own,
    run: Run,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TrashOut {
    item: Nonce,
    entry: EntryHash,
    at: Hlc,
    from: RelPath,
    identity: Identity,
    len: u64,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ContributionOut {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    entities: BTreeMap<EntityId, EntityOut>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    dismissals: Vec<DismissalOut>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    trash: Vec<TrashOut>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    settled: Vec<(WriterId, Nonce)>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    unknown: Vec<(EntryHash, Raw)>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    extended: Vec<(EntryHash, Raw)>,
}

impl HeldOut {
    fn of(field: &FieldKey, held: &Held) -> Option<Self> {
        let (set, off, pin) = match &held.value {
            Value::Exist(created) => (None, !created, false),
            Value::Register(value) => (value.clone(), value.is_none(), false),
            Value::Member(added) => (None, !added, false),
            Value::File { fact, pin } => {
                let fact = match fact {
                    Some(fact) => Some(Raw::new(&serde_json::to_string(fact).ok()?).ok()?),
                    None => None,
                };
                (fact.clone(), fact.is_none(), *pin)
            }
        };
        let FieldOut { key, value, file } = FieldOut::of(field);
        Some(Self {
            key,
            value,
            file,
            at: held.at,
            stamp: held.stamp,
            first: held.first,
            set,
            off,
            pin,
        })
    }

    fn read(self, entity: EntityId) -> Option<(FieldKey, Held)> {
        let field = FieldOut::read(self.key, self.value, self.file).field(entity)?;
        let value = match (&field, self.set, self.off, self.pin) {
            (FieldKey::Exist(_), None, off, false) => Value::Exist(!off),
            (FieldKey::Register(..), Some(value), false, false) => Value::Register(Some(value)),
            (FieldKey::Register(..), None, true, false) => Value::Register(None),
            (FieldKey::Member(..), None, off, false) => Value::Member(!off),
            (FieldKey::File(_), Some(fact), false, pin) => Value::File {
                fact: Some(fact.decode().ok()?),
                pin,
            },
            (FieldKey::File(_), None, true, false) => Value::File {
                fact: None,
                pin: false,
            },
            _ => return None,
        };
        let held = Held {
            at: self.at,
            stamp: self.stamp,
            value,
            first: self.first,
        };
        Some((field, held))
    }
}

impl Serialize for Contribution {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut out = ContributionOut::default();
        for (field, held) in &self.own {
            let held = HeldOut::of(field, held)
                .ok_or_else(|| serde::ser::Error::custom("a file fact that is not JSON"))?;
            out.entities
                .entry(field.entity())
                .or_default()
                .writes
                .push(held);
        }
        for ((field, _, above), reference) in &self.refs {
            let FieldOut { key, value, file } = FieldOut::of(field);
            out.entities
                .entry(field.entity())
                .or_default()
                .refs
                .push(ReferenceOut {
                    key,
                    value,
                    file,
                    above: *above,
                    own: reference.own,
                    target: reference.target,
                });
        }
        for (field, own) in &self.killed {
            let FieldOut { key, value, file } = FieldOut::of(field);
            out.entities
                .entry(field.entity())
                .or_default()
                .killed
                .push(KilledOut {
                    key,
                    value,
                    file,
                    own: *own,
                });
        }
        for ((field, writer), taken) in &self.takes {
            let FieldOut { key, value, file } = FieldOut::of(field);
            out.entities
                .entry(field.entity())
                .or_default()
                .takes
                .push(TakenOut {
                    key,
                    value,
                    file,
                    writer: *writer,
                    own: taken.own,
                    below: taken.below,
                });
        }
        out.dismissals = self
            .dismissals
            .iter()
            .map(|(own, run)| DismissalOut {
                own: *own,
                run: *run,
            })
            .collect();
        out.trash = self
            .trash
            .iter()
            .map(|(item, trashed)| TrashOut {
                item: *item,
                entry: trashed.entry,
                at: trashed.at,
                from: trashed.from.clone(),
                identity: trashed.identity,
                len: trashed.len,
            })
            .collect();
        out.settled = self.settled.iter().copied().collect();
        out.unknown = self.unknown.iter().cloned().collect();
        out.extended = self.extended.iter().cloned().collect();
        let later = self
            .later
            .iter()
            .filter(|(name, _)| !STATE.contains(&name.as_str()))
            .map(|(name, raw)| (name.as_str(), raw))
            .collect();
        WrittenState { out, later }.serialize(serializer)
    }
}

/// A contribution's members, as [`ContributionOut`] names them.
const STATE: [&str; 6] = [
    "entities",
    "dismissals",
    "trash",
    "settled",
    "unknown",
    "extended",
];

#[derive(Serialize)]
struct WrittenState<'a> {
    #[serde(flatten)]
    out: ContributionOut,
    #[serde(flatten)]
    later: BTreeMap<&'a str, &'a Raw>,
}

impl<'de> Deserialize<'de> for Contribution {
    /// ⚠️ Reads the members as text first: `#[serde(flatten)]` buffers the
    /// [`Raw`] values of the members it keeps.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut members: BTreeMap<String, Raw> = BTreeMap::deserialize(deserializer)?;
        let known: BTreeMap<String, Raw> = STATE
            .iter()
            .filter_map(|name| Some((name.to_string(), members.remove(*name)?)))
            .collect();
        let text = serde_json::to_string(&known).map_err(D::Error::custom)?;
        let out: ContributionOut = serde_json::from_str(&text).map_err(D::Error::custom)?;
        let bad = |what: &str| D::Error::custom(format!("a contribution with {what}"));
        let mut contribution = Self::default();
        for (entity, record) in out.entities {
            for held in record.writes {
                let (field, held) = held
                    .read(entity)
                    .ok_or_else(|| bad("a write naming no one field"))?;
                if contribution.own.insert(field, held).is_some() {
                    return Err(bad("two writes of one field"));
                }
            }
            for reference in record.refs {
                let field = FieldOut::read(reference.key, reference.value, reference.file)
                    .field(entity)
                    .ok_or_else(|| bad("a reference naming no one field"))?;
                let key = (field, reference.target.writer, reference.above);
                let kept = Reference {
                    own: reference.own,
                    target: reference.target,
                };
                if contribution.refs.insert(key, kept).is_some() {
                    return Err(bad("two references in one place"));
                }
            }
            for killed in record.killed {
                let field = FieldOut::read(killed.key, killed.value, killed.file)
                    .field(entity)
                    .ok_or_else(|| bad("a delete naming no one field"))?;
                if contribution.killed.insert(field, killed.own).is_some() {
                    return Err(bad("two deletes of one field"));
                }
            }
            for taken in record.takes {
                let field = FieldOut::read(taken.key, taken.value, taken.file)
                    .field(entity)
                    .ok_or_else(|| bad("a take naming no one field"))?;
                let kept = Taken {
                    own: taken.own,
                    below: taken.below,
                };
                if contribution
                    .takes
                    .insert((field, taken.writer), kept)
                    .is_some()
                {
                    return Err(bad("two takes in one place"));
                }
            }
        }
        contribution.dismissals = out.dismissals.into_iter().map(|d| (d.own, d.run)).collect();
        for item in out.trash {
            let trashed = Trashed {
                entry: item.entry,
                at: item.at,
                from: item.from,
                identity: item.identity,
                len: item.len,
            };
            if contribution.trash.insert(item.item, trashed).is_some() {
                return Err(bad("a trash item twice"));
            }
        }
        contribution.settled = out.settled.into_iter().collect();
        contribution.unknown = out.unknown.into_iter().collect();
        contribution.extended = out.extended.into_iter().collect();
        contribution.later = members;
        Ok(contribution)
    }
}

// The packing of a contribution, as an install keeps it for itself.

impl Pack for FieldKey {
    fn pack(&self, out: &mut Vec<u8>) {
        match self {
            Self::Exist(entity) => {
                out.push(0);
                entity.pack(out);
            }
            Self::Register(entity, key) => {
                out.push(1);
                (entity, &**key).pack(out);
            }
            Self::Member(entity, key, value) => {
                out.push(2);
                (entity, (&**key, value)).pack(out);
            }
            Self::File(entity) => {
                out.push(3);
                entity.pack(out);
            }
        }
    }
}

impl Unpack for FieldKey {
    fn unpack(input: &mut In<'_>) -> Unpacked<Self> {
        Ok(match input.byte()? {
            0 => Self::Exist(Unpack::unpack(input)?),
            1 => {
                let (entity, key): (EntityId, String) = Unpack::unpack(input)?;
                Self::Register(entity, name(&key))
            }
            2 => {
                let (entity, (key, value)): (EntityId, (String, Raw)) = Unpack::unpack(input)?;
                Self::Member(entity, name(&key), value)
            }
            3 => Self::File(Unpack::unpack(input)?),
            _ => return bad_variant(),
        })
    }
}

impl Pack for Value {
    fn pack(&self, out: &mut Vec<u8>) {
        match self {
            Self::Exist(created) => {
                out.push(0);
                created.pack(out);
            }
            Self::Register(value) => {
                out.push(1);
                value.pack(out);
            }
            Self::Member(added) => {
                out.push(2);
                added.pack(out);
            }
            Self::File { fact, pin } => {
                out.push(3);
                (fact, pin).pack(out);
            }
        }
    }
}

impl Unpack for Value {
    fn unpack(input: &mut In<'_>) -> Unpacked<Self> {
        Ok(match input.byte()? {
            0 => Self::Exist(Unpack::unpack(input)?),
            1 => Self::Register(Unpack::unpack(input)?),
            2 => Self::Member(Unpack::unpack(input)?),
            3 => {
                let (fact, pin) = Unpack::unpack(input)?;
                Self::File { fact, pin }
            }
            _ => return bad_variant(),
        })
    }
}

crate::pack::pack_struct!(Held {
    at,
    stamp,
    value,
    first
});
crate::pack::pack_struct!(Reference { own, target });
crate::pack::pack_struct!(Taken { own, below });
crate::pack::pack_struct!(Trashed {
    entry,
    at,
    from,
    identity,
    len
});

impl Pack for Contribution {
    fn pack(&self, out: &mut Vec<u8>) {
        self.own.pack(out);
        let refs = self.refs.iter();
        let refs: Vec<_> = refs
            .map(|((f, w, above), r)| ((f, (w, above)), r))
            .collect();
        refs.pack(out);
        self.killed.pack(out);
        let takes: Vec<_> = self.takes.iter().map(|((f, w), t)| ((f, w), t)).collect();
        takes.pack(out);
        self.dismissals.iter().collect::<Vec<_>>().pack(out);
        self.trash.pack(out);
        self.settled.iter().collect::<Vec<_>>().pack(out);
        self.unknown.iter().collect::<Vec<_>>().pack(out);
        self.extended.iter().collect::<Vec<_>>().pack(out);
        self.later.pack(out);
    }
}

/// Packed as a sequence of pairs.
type Pairs<K, V> = Vec<(K, V)>;

impl Unpack for Contribution {
    fn unpack(input: &mut In<'_>) -> Unpacked<Self> {
        let own = Unpack::unpack(input)?;
        let refs: Pairs<(FieldKey, (WriterId, bool)), Reference> = Unpack::unpack(input)?;
        let killed = Unpack::unpack(input)?;
        let takes: Pairs<(FieldKey, WriterId), Taken> = Unpack::unpack(input)?;
        let dismissals: Vec<(Own, Run)> = Unpack::unpack(input)?;
        let trash = Unpack::unpack(input)?;
        let settled: Vec<(WriterId, Nonce)> = Unpack::unpack(input)?;
        let unknown: Vec<(EntryHash, Raw)> = Unpack::unpack(input)?;
        let extended: Vec<(EntryHash, Raw)> = Unpack::unpack(input)?;
        let later = Unpack::unpack(input)?;
        Ok(Self {
            own,
            refs: refs
                .into_iter()
                .map(|((field, (writer, above)), reference)| ((field, writer, above), reference))
                .collect(),
            killed,
            takes: takes.into_iter().collect(),
            dismissals: dismissals.into_iter().collect(),
            trash,
            settled: settled.into_iter().collect(),
            unknown: unknown.into_iter().collect(),
            extended: extended.into_iter().collect(),
            later,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log::{Bound, Genesis, Logged};

    fn hash(n: u128) -> EntryHash {
        EntryHash::from_u128(n)
    }

    fn at(n: u64) -> Hlc {
        Hlc {
            wall_ms: n,
            counter: 0,
        }
    }

    fn raw(json: &str) -> Raw {
        Raw::new(json).unwrap()
    }

    const ME: WriterId = WriterId::from_u128(1);
    const THEM: WriterId = WriterId::from_u128(2);
    const E: EntityId = EntityId::from_u128(7);

    fn them(n: u64) -> Ref {
        Ref {
            writer: THEM,
            n,
            hash: hash(100 + u128::from(n)),
        }
    }

    /// `ops` logged one entry each, after a genesis entry.
    fn chain(ops: Vec<(Op, Vec<Forked>)>) -> Vec<Entry> {
        let genesis = Entry::encode(
            EntryHash::ZERO,
            0,
            at(0),
            EntryKind::Genesis(Genesis {
                writer: ME,
                label: "me".into(),
            }),
        )
        .unwrap();
        let mut entries = vec![genesis];
        for (op, forks) in ops {
            let prev = entries.last().unwrap();
            let kind = EntryKind::Intent(Logged {
                label: "L".into(),
                ops: vec![op],
                displaced: vec![],
                forks,
                more: 0,
            });
            entries.push(Entry::encode(prev.hash(), prev.n() + 1, at(prev.n() + 1), kind).unwrap());
        }
        entries
    }

    fn folded(entries: &[Entry]) -> Contribution {
        let mut contribution = Contribution::default();
        for entry in entries {
            contribution.fold(ME, entry);
        }
        contribution
    }

    #[test]
    fn a_later_own_write_replaces_the_held_one_and_keeps_its_first_number() {
        let write = |value: &str| Op::Write {
            entity: E,
            key: "k".into(),
            value: Some(raw(value)),
            replaces: vec![],
            take: None,
        };
        let entries = chain(vec![(write("1"), vec![]), (write("2"), vec![])]);
        let contribution = folded(&entries);
        let held = &contribution.own[&FieldKey::Register(E, "k".into())];
        assert_eq!((held.at, held.first), (entries[2].own(), 1));
        assert_eq!(held.value, Value::Register(Some(raw("2"))));
    }

    #[test]
    fn references_are_kept_one_per_field_writer_and_side_of_a_known_fork() {
        let write = |replaces: Vec<Ref>| Op::Write {
            entity: E,
            key: "k".into(),
            value: Some(raw("1")),
            replaces,
            take: None,
        };
        let fork = Forked {
            writer: THEM,
            at: 2,
        };
        let entries = chain(vec![
            (write(vec![them(1)]), vec![]),
            (write(vec![them(2), them(4)]), vec![fork]),
            (write(vec![them(3)]), vec![fork]),
        ]);
        let contribution = folded(&entries);
        let field = FieldKey::Register(E, "k".into());
        let below = contribution.refs[&(field.clone(), THEM, false)];
        let above = contribution.refs[&(field, THEM, true)];
        assert_eq!((below.target, below.own), (them(2), entries[2].own()));
        assert_eq!((above.target, above.own), (them(4), entries[2].own()));
    }

    #[test]
    fn an_own_delete_is_kept_beside_each_earlier_own_write_of_its_entity() {
        let set = |key: &str| Op::Write {
            entity: E,
            key: key.into(),
            value: Some(raw("1")),
            replaces: vec![],
            take: None,
        };
        let delete = Op::Delete {
            entity: E,
            replaces: vec![],
            observed: vec![],
            take: None,
        };
        let entries = chain(vec![
            (set("a"), vec![]),
            (delete.clone(), vec![]),
            (set("b"), vec![]),
            (delete, vec![]),
        ]);
        let contribution = folded(&entries);
        assert_eq!(
            contribution.killed[&FieldKey::Register(E, "a".into())],
            entries[2].own()
        );
        assert_eq!(
            contribution.killed[&FieldKey::Register(E, "b".into())],
            entries[4].own()
        );
    }

    #[test]
    fn a_contribution_reads_back_as_written_in_json_and_packed() {
        let fact = FileFact {
            path: RelPath::new("a.syx").unwrap(),
            identity: Identity::from_u128(3),
            len: 4,
            modified: Some(5),
        };
        let take = Run {
            writer: THEM,
            from: 3,
            first: Some(hash(103)),
            last: them(4).own(),
            tip: None,
        };
        let entries = chain(vec![
            (
                Op::Create {
                    entity: E,
                    replaces: vec![them(1)],
                    take: None,
                },
                vec![],
            ),
            (
                Op::Write {
                    entity: E,
                    key: "k".into(),
                    value: None,
                    replaces: vec![them(2)],
                    take: Some(take),
                },
                vec![Forked {
                    writer: THEM,
                    at: 2,
                }],
            ),
            (
                Op::Add {
                    entity: E,
                    key: "tags".into(),
                    value: raw("null"),
                    replaces: vec![],
                    take: None,
                },
                vec![],
            ),
            (
                Op::Remove {
                    entity: E,
                    key: "tags".into(),
                    value: raw(r#""x""#),
                    tags: vec![them(3)],
                    take: None,
                },
                vec![],
            ),
            (
                Op::File {
                    entity: E,
                    file: Some(fact.clone()),
                    replaces: vec![],
                    take: None,
                },
                vec![],
            ),
            (
                Op::Delete {
                    entity: E,
                    replaces: vec![],
                    observed: vec![Observed {
                        target: Target::Member("tags".into(), raw(r#""y""#)),
                        at: them(5),
                    }],
                    take: None,
                },
                vec![],
            ),
            (Op::Dismiss { runs: vec![take] }, vec![]),
            (Op::Unknown(raw(r#"{"op":"later"}"#)), vec![]),
        ]);
        let mut contribution = folded(&entries);
        let bind = Entry::encode(
            entries.last().unwrap().hash(),
            9,
            at(9),
            EntryKind::Bind(Bound {
                ops: vec![Op::Pin {
                    entity: EntityId::from_u128(8),
                    file: fact,
                    replaces: vec![],
                    take: None,
                }],
            }),
        )
        .unwrap();
        contribution.fold(ME, &bind);
        contribution.settled.insert((THEM, Nonce::from_u128(4)));
        let json = serde_json::to_string(&contribution).unwrap();
        let read: Contribution = serde_json::from_str(&json).unwrap();
        assert_eq!(read, contribution, "{json}");
        let mut packed = Vec::new();
        contribution.pack(&mut packed);
        let mut input = In::new(&packed);
        assert_eq!(Contribution::unpack(&mut input), Ok(contribution));
        assert!(input.end().is_ok());
    }

    #[test]
    fn a_contribution_naming_one_place_twice_is_refused() {
        let e = "00000000000000000000000000000007";
        let w = "00000000000000000000000000000002";
        let h = "00000000000000000000000000000003";
        for json in [
            format!(
                r#"{{"entities":{{"{e}":{{"writes":[{{"key":"k","at":[1,"{h}"],"stamp":[1,0],"first":1,"off":true}},{{"key":"k","at":[2,"{h}"],"stamp":[1,0],"first":1,"off":true}}]}}}}}}"#
            ),
            format!(
                r#"{{"entities":{{"{e}":{{"writes":[{{"key":"k","file":true,"at":[1,"{h}"],"stamp":[1,0],"first":1}}]}}}}}}"#
            ),
            format!(
                r#"{{"entities":{{"{e}":{{"refs":[{{"own":[1,"{h}"],"target":["{w}",1,"{h}"]}},{{"own":[2,"{h}"],"target":["{w}",1,"{h}"]}}]}}}}}}"#
            ),
        ] {
            assert!(
                serde_json::from_str::<Contribution>(&json).is_err(),
                "{json}"
            );
        }
    }
}
