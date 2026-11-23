# RFC-0360: Relations between files

- Start date: 2026-11-22
- Authors: jmoo
- RFC PR: [jmoo/drawbar#360](https://github.com/jmoo/drawbar/pull/360)
- Tracking issue: none; the RFC PR implements it

## Summary

toshokan gains relations. A relation is a register or a set whose values each name
another entity by its id, such as the recordings a piano library was built from. A
view says which entity each value names now, lists the entities that name a given
one, and reports a value whose target is gone without dropping it. When two entities
turn out to be one file and are joined, the delete that ends one says which entity it
was joined into, so what named either still names the file. A relation holds no path
and no file name. The format gains one optional member of the `delete` op and one
member of a snapshot's state; the merge is as it was. drawbar declares no relation
in this change; the mechanism is here for the features that need one.

## Motivation

RFC-0271 said the library would keep relations between files, described how they
should behave, and left the work open: "toshokan does not yet treat entity references
specially: a link is the app's JSON like any other value. Indexing links by target,
resolving targets in the view and reporting dangling ones is the work this needs."
It also left open whether toshokan or the app should do it.

drawbar ties files together today by path and by name, or not at all:

| Tie | How it is kept today | What breaks it |
|---|---|---|
| A Sample Editor project (`.nsmpproj`) and its WAVs | A path in the project file, relative to the project | Renaming or moving a WAV, or moving the project, shows "N WAVs missing" |
| A piano library (`.npno`) or sample (`.nsmp`) and the WAVs it was built from | Not kept | The library cannot be rebuilt, and nothing says which recordings are still in use |
| A piano library and the library it took as a template | Not kept | — |

RFC-0276 lists "linking a built npno back to its takes" as future work, and building
a library from several recordings ([#351](https://github.com/jmoo/drawbar/pull/351))
needs to say which recordings belong to which library. Each of these could invent a
file that lists its members by path. That file would have to follow every rename and
move, inside drawbar and outside it, and two computers editing it would be the
`library.ron` problem again.

The store already has what a relation needs: an entity's id is random and never
reused, and binding follows a file through renames and moves made outside the app.
An app can store an id in a value today. Three things are missing:

- **A lookup by target.** `View::find` matches a whole value, so a value that holds
  more than the id cannot be found by the id.
- **A report of a target that is gone.** Nothing tells a deleted target from one
  that is there.
- **A target that survives a join.** Two computers can each make an entity of one
  file before either sees the other's. Binding gives the file to the lower id, and
  the app joins the other into it, which deletes it. A value that named the deleted
  one then names nothing. Relations make this common: naming a file that has no
  entity creates one, so two computers that each build from the same recording have
  made two entities for it.

The third cannot be done by the app alone. A join is committed by whichever drawbar
sees the pair first, and it cannot fix a value written on another computer that it
has not seen yet.

## Guide-level explanation

### What a contributor sees

A relation is an ordinary key. Its value type says which entity a value names:

```rust
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
struct Source { to: EntityId, role: Role }

impl Relates for Source {
    fn target(&self) -> EntityId { self.to }
}

const BUILT_FROM: Set<Source> = Set::new("built from");
const TEMPLATE: Register<EntityId> = Register::new("template");
```

An `EntityId` names itself, so the simplest relation is a `Set<EntityId>` or a
`Register<EntityId>`. A set is a relation to many entities; a register is a relation
to one, and two targets set on two computers show as a conflict, as any register
does.

Writing a relation is writing a value. One intent can create the entities it names:

```rust
let (build, take) = lib.intent("Build").create(|e| {
    e.adopt(&wav, Expect::Listed { len, modified });
});
build.add(piano, BUILT_FROM, Source { to: take, role }).commit()?;
```

Reading it resolves each value:

```rust
let view = lib.view();
for related in view.entity(piano).unwrap().relations(BUILT_FROM).related {
    match related.to.and_then(|to| view.entity(to)) {
        Some(take) => show(take.file()),
        None => show_missing(&related.value),
    }
}
let used_by = view.referrers(BUILT_FROM, take);
```

- `View::resolve(id)` is the entity an id names now, or `None`.
- `EntityView::relations(key)` is the members of one entity's set with what each
  names, and `View::relations(key)` every entity's values.
- `View::referrers(key, target)` is the entities that name `target`.

### What a relation survives

| What happens | What the relation shows |
|---|---|
| The target or the holder is renamed or moved, in the app or outside it | The same entity, at its new path |
| The target is edited in place by another program | The same entity; its file is reported changed outside |
| The holder's file is saved over | The same values; they belong to the entity, not to its bytes |
| The target is deleted | The value stays and resolves to nothing. Undo, or a revive, resolves it again |
| The holder is deleted | Its values go with it, and come back with undo |
| The holder syncs before the target's entity does | The value resolves to nothing until the target's entries arrive |
| The target's entity is there and its file is not | The entity, with a missing file |
| Two computers add values | Both are kept. A removal takes only the values it saw |
| The target turns out to be the same file as another entity | The entity that has the file, before and after the join is logged |

A file copied outside the app has no entity and so no relations, as RFC-0271's copy
rule says. Whether a duplicate made in the app carries its relations is the app's
choice: it writes them or it does not.

## Reference-level explanation

### Values

`toshokan::Relates` is the one new trait:

```rust
pub trait Relates: Value {
    fn target(&self) -> EntityId;
}
```

Nothing is declared in the schema, and nothing marks a relation in the log. A
relation's values are the app's JSON, written by `set`, `add` and `remove`, merged,
compacted and undone as any other value. The lookups take the typed key, so they
decode each value as the key's type to find its target.

A value names one entity. What else it holds is the app's: a role, where the
relation was learned, or nothing. A set's member is its whole JSON text, so two
members that name one target with different data are two members, and both are
shown. Data that changes often belongs in a register beside the relation, not in a
set's members: a set keeps a record of every member it ever removed (see Drawbacks).

A relation that may gain data later is best a struct from the start, even with the
one field `to`. A field added later that is left out of the JSON when unset keeps
every member written before it equal to the same member written after.

A target that is deleted has nothing left to show but the value that names it: a
delete ends the entity's fields. An app that wants to say what a missing target was
keeps a name in the value.

`EntityView::relations` reads a set. A register is read with `EntityView::get`,
which says whether it holds one value or a conflict, and `View::resolve`.
`View::relations` and `View::referrers` take either, and read each side of a
register in conflict.

### Resolving

An id resolves to an entity the view shows, or to none:

1. The id of an entity shown resolves to that entity.
2. The id of an entity not shown resolves as the id its join names. A delete logged
   by a join names the entity joined into; where two live deletes name different
   entities, the later is followed. The id resolves to none where no live delete of
   it names an entity, and where the joins lead back to an id already passed.
3. Where binding reports the entity of step 1 or 2 joined into another
   (`View::joined`), the id resolves to that other.

Step 3 covers the time between two computers adopting one file and a drawbar logging
the join. Step 2 covers the time after. Step 2 follows one delete at each entity, so
it is a walk that ends, and what it finds is what one delete can say: undo, which
logs one delete, puts back the join the return replaced. Binding joins only into the
entity bound to the file, so step 3 is one step.

An entity that is shown resolves to itself, or to the entity a scan joins it into. A
joined entity brought back is shown again, and so is one in a deletion conflict,
until the conflict is settled.

### What a join records

A join already deletes the joined entity. Its `delete` op now names the entity
joined into:

```json
{"op":"delete","entity":"<B>","replaces":[…],"observed":[…],"into":"<A>"}
```

The fact lives in the existence write it describes, so the rules for existence
writes carry it without a rule of its own:

| What happens after `B` is joined into `A` | What `B`'s id resolves to |
|---|---|
| Nothing | `A` |
| `A` is joined into `C` | `C`, through both deletes |
| `A` is deleted | None. Undoing that delete resolves it to `A` again |
| `B` is brought back | `B`: the create replaced the delete |
| `B` is brought back and deleted | None: the later delete is not a join |
| `B` is brought back, and that is undone | `A`: the undo's delete names what the replaced one named |
| An intent that joined and did something else is undone, then redone | `B`, then `A` |
| Two writers join `B` into `A` at once | `A`: both deletes say so |
| Two writers join `B` into `A` and into `C` at once | The one the later join names. If that one is deleted, none |
| One writer joins `B` into `A` while another joins `A` into `C` | `C`, or `A` kept in a deletion conflict where it gained something its delete did not see |
| Two writers join `A` into `B` and `B` into `A` at once | None for both, unless what one gained keeps it shown in a deletion conflict |

Only a join writes `into` on its own account. Three deletes carry one forward: the
delete that undoes the return of a joined entity, the delete that redoes a join, and
the delete that takes a join from a history kept apart (`FactChange::Delete` carries
it). Any other delete names no entity.

Undo refuses to reverse a delete once another writer's delete of the same entity
says otherwise: it names another entity, or one where this named none, or none where
this named one. It refuses in the same way to reverse a write another writer has
changed since.

One intent joins an entity once. It may not delete or join away an entity it joins
or joins into, nor bring back one it joins. Such a plan is refused: a join would give
fields to an entity the same entry deletes, or the entry would write one entity's
existence twice.

A snapshot keeps, per field, one record whose members are frozen, so the entity
joined into goes in a new member of snapshot state, `joined`, beside the records:
one entry per delete that names one. A build that does not know `joined` keeps it
as it keeps any member of state it does not know, and a reader ignores an entry
whose delete the snapshot no longer holds.

### Lookups and their cost

Lookups read the merged state the view already holds. No index is stored or kept
between views, and a view computes nothing for relations until one is asked for.

Measured on a release build, for a library of 2,020 entities where 20 hold 300
values each of one relation (6,000 values):

| Call | Time |
|---|---|
| `EntityView::relations`, one holder's 300 values | 46 µs |
| `View::relations`, all 6,000 values | 0.91 ms |
| `View::referrers`, one target | 0.75 ms |
| `View::find` on a register every entity holds, before this change | 0.34 ms |
| The same `View::find`, after | 0.06 ms |

`referrers` visits every value of the key. `find`, `with` and `values` visit every
entity, and now read only the key asked for, where they used to list every value of
each entity first: `with` and `values` on a set every entity holds went from 0.33 ms
and 0.36 ms to 0.08 ms and 0.11 ms. An app that asks for many targets at once reads
`View::relations` once and indexes it, as drawbar already mirrors each view into its
own model.

### What a change does not list

`View::changed_since` lists the entities whose own state differs. A target that is
deleted, joined or brought back is listed, shown or not; the entities that name it
are not, since nothing of theirs changed. An app that shows relations redraws, for
each entity listed, its referrers in the earlier view and in the new one: an entity
that names a target through a joined id is found only in the view that resolves it.

### The format

Two members are new, and both are additive under the format's compatibility rules:

- `into` on the `delete` op.
- `joined` in a snapshot's `state`.

The line form, the other ops, the merge rule, compaction and retention are as they
were: `into` is part of an existence write's value and changes nothing about which
write is live. A build from before this change reads a library this build wrote,
shows the same entities, and keeps both members. It folds a `delete` with `into` as
a delete and keeps the whole entry, and a newer reader of its snapshot reads `into`
from that entry. A new reader takes an `into` that is not an id for no `into`, so
that it shows the same entities as a build that ignores the member.

A join logged by a build from before this change names no entity, so the id it
deleted resolves to none (see Drawbacks).

### drawbar

drawbar declares no relation and does not change. It already commits a join for
each pair a scan reports, so its joins record `into` from the first release built
against this toshokan, ahead of any feature that writes a relation.

### Evidence

- `tests/facts.rs` drives two and three writers through the real merge, reading
  from the log and again from a snapshot where compaction could differ. It covers
  each row of the table under "What a join records", the plans and undos that are
  refused, a deleted target and a deleted holder with their undo, a target whose
  writer has not arrived, values that do not decode, a register in conflict, and
  the redraw rule. Renames, moves and edits outside the app are binding's, and its
  suites cover them.
- The same file runs 200 seeded walks of three writers that create, name, join,
  delete, bring back and sync at random. Once all hold the same entries, every
  writer and a reader of their snapshots resolve every id alike, to an entity they
  show, and `referrers` returns exactly the holders that the relation's rows give.
- `tests/library.rs` runs relations through a real library on both drivers and on
  copying folders: two writers adopt one file and every reader resolves the higher
  entity to the one bound to the file before any join is logged and after; an
  entity joined into one that a scan then joins; and a join taken from a forked
  history.
- Unit tests of the snapshot state cover an entry of `joined` whose delete is no
  longer held, two entries for one entity, and a join that an older build kept as a
  whole entry, read as its last write of the entity says. A unit test of the log
  covers a `delete` whose `into` is not an id.
- A new format fixture, `one-writer-joined`, holds a snapshot with `joined` and a
  log line whose `delete` has `into`. Every later build must read it as its oracle
  says and encode it back byte for byte.
- The existing suites pass, the retention sweep and power-loss workloads included.

## Drawbacks

- **A removed value leaves a record.** A set keeps, per writer, a record of each
  member it removed, so a relation whose values are rewritten often grows the
  snapshot. This is the existing rule for sets, and tags have the same cost. It is
  the reason changing data belongs in a register.
- **A value that changes shape is a different member.** Adding a field to a value
  type makes new members differ from old ones by text. Fields that serialize only
  when set keep old members equal. Removing a member a newer build wrote, with
  fields this build drops, needs a removal by stored text, which
  [#325](https://github.com/jmoo/drawbar/pull/325) adds.
- **Nothing checks a target when it is written.** A relation may name any id. An id
  that never existed reads the same as a deleted one: unresolved.
- **Lookups by target visit every value.** See the measurements. A view could keep
  an index per key if an app needs one.
- **A join is one fact on one delete.** Settling a deletion conflict on a joined
  entity by deleting it again is a plain delete, and what named it then resolves to
  none. Joining it again keeps what it names.
- **Older builds.** A join by a build from before this change records nothing. Where
  an older drawbar is the one that joins a pair, what a newer one wrote naming the
  joined entity resolves to none, and nothing repairs it: the value is kept, and the
  user names the file again. This is why the join records `into` now, before drawbar
  writes any relation. An older build that finishes a newer build's interrupted
  commit writes its ops again without `into`; that needs a join in an intent that
  also moves files, which drawbar does not commit.
- **A join carries the keys its app declares.** An app whose schema lacks a relation's
  key joins an entity without carrying that relation to the survivor.
  [#325](https://github.com/jmoo/drawbar/pull/325) changes `join` to carry every key.
- **A join kept apart is offered while its entity is shown.** Where a forked history
  holds a join and the merged state has already deleted that entity some other way,
  the join is not listed among the changes to take, as no delete of an entity
  already gone is.

## Rationale and alternatives

The design follows from one choice: a relation's meaning lives in the app's value
type, and toshokan adds only what the app cannot do, which is following an entity
through a join.

| Alternative | Why not |
|---|---|
| The app resolves ids itself with `View::find` and scans for referrers | It cannot follow a join, for the reason Motivation gives. Every app would write the same lookups |
| A reference kind declared in the schema, with an index the view maintains | The typed key already says how to read a target. An index is derived state to build on every view, and the measurements do not call for it |
| The join rewrites every value that names the joined entity | It sees only the values its writer has read, so one written elsewhere at the same time is left behind. It also needs toshokan to edit values it does not own |
| Follow every live join of an entity and take the lowest entity shown | This was the second design here. With two joins into different entities, what an id resolved to changed when a scan's join was logged, and changed again when an unrelated entity was joined. Undo writes one delete and could not put back two. Following the later join alone has neither problem, and is how a register in conflict already picks what it shows |
| The survivor keeps a set of the ids joined into it, under a key toshokan reserves | This was the first design here, and review took it apart. The fact sat on the survivor, apart from the delete it describes, so nothing ended it: after the joined entity was brought back and deleted for good, the survivor still answered for its id. The member was a field write on the survivor, so a join concurrent with the survivor's delete left the survivor shown in a deletion conflict over nothing a user did. Taking a forked join took the delete and the member as two changes that could be taken apart. It reserved a key name in every schema, and each later join copied every id again |
| A new op or field kind for relations, such as a map from target to data | A second merge rule, snapshot record and retention term beside sets and registers. Sets and registers already merge the way a relation should |
| Refuse a value whose target is not shown | Undoing a removal, taking a change from a kept-apart history, and naming a target whose entries have not arrived are all correct writes of a target not shown. An unresolved target is reported, never silent |
| Values that name several entities, such as an ordered list in one register | No caller needs it. It can be added when one does |
| A file that lists members by path, or a sidecar per file | RFC-0271 rejected sidecars for the same reasons: it must follow every rename, and two writers editing one file is the problem the store exists to remove |
| One mechanism for these and the links of [#325](https://github.com/jmoo/drawbar/pull/325) | See below |

**Relations and #325's links are different things.** A link in RFC-0325 ties a file
to a slot on a keyboard: a place outside the library, named by a keyboard and an
address, carrying the checksums the two last shared. A relation ties an entity to an
entity. The link has nothing to resolve in the store, and the relation has no state
to compare. Both are values in a set per file, which the store already provides, and
neither needs the other's code. #325 also changes `join` to carry every key, declared
or not. The two changes meet in that one function and do not conflict: this one
changes only the delete the join ends with. This RFC uses "relation" and "target",
and leaves "link" to RFC-0325.

**If nothing is done**, each feature that ties files together stores paths or names,
and the take a library was built from is lost the first time someone tidies a folder.

## Prior art

- RFC-0271, "Relations between files", which this implements. It differs in one
  place: RFC-0271 expected a lookup by target to need an index, and a typed lookup
  over the merged state proved enough.
- Unity references assets by a GUID kept in a `.meta` file beside each asset, and
  Godot by a `uid://` kept in a sidecar or the file's header. Both survive renames
  made in the editor. Both lose or duplicate the id when the sidecar does not follow
  the file, which is why the id here lives in the store and binding finds the file.
- Obsidian links notes by path and rewrites every referrer on a rename made inside
  the app. A rename outside it breaks the link.
- MediaWiki keeps a redirect at the old title when two pages are merged, so links
  written before the merge, or by someone who has not seen it, still arrive. A
  delete's `into` is that redirect, kept at the old title.

## Unresolved questions

- Whether a plain delete that settles a deletion conflict on a joined entity should
  keep the `into` of the delete it replaces. It does not here: a delete says only
  what its intent said.
- Whether a view should say that an entity shown was joined by another writer, so
  an app other than drawbar can offer to join it again.

## Future possibilities

- drawbar's first relations: a built `.npno` or `.nsmp` and the WAVs it was built
  from, a Sample Editor project and its WAVs, and a library and its template. Each
  needs a key name and a value shape, which are fixed once a release writes them.
- A warning before deleting a file that others name, and export that collects what
  a file names.
- What a bundle's manifest says its programs play
  ([#183](https://github.com/jmoo/drawbar/issues/183)), recorded at unpack.
- An index by target kept per view, if an app's lookups need it.
