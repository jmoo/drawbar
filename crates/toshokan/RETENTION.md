# Bounded retention: a protocol revision (draft)

This draft revises how much history toshokan keeps. It will be folded into
[SPEC.md](SPEC.md) and the model in [spec/](spec/) once settled. Nothing here is
implemented.

## Decisions this serves

- **D10.** Keep only as much history as stops two instances stomping on each
  other. History is not for version-control features.
- **D11.** Undo and redo reach only this session's intents and end at close.
- **D12.** After a folder is restored from an older copy, the folder wins: no
  retained view of restored-away facts, no let go or adopt.
- **D13.** Unsaved edits stay local to each install.
- **D14.** A write whose basis was forgotten is shown as a conflict, never
  resolved silently either way. There is no offline time horizon.

## The problem

At a constant 2,000 entities, cost grows linearly with history. With
`examples/scale.rs --entities 2000 --files 2000 --writers 3`:

| Entries | Memory (open cold) | Folder | Cached view |
| ------: | -----------------: | -----: | ----------: |
|     400 |             4.5 MB | 1.8 MB |      2.0 MB |
|      2M |             767 MB | 505 MB |      286 MB |

Compaction saves about a quarter and no memory, because a snapshot keeps every
folded hash and every register keeps every write it ever replaced.

## What history protects today

Each row was checked in the code and the spec. The last column is what this
design does with it.

| History kept | Where | What it protects | Here |
| --- | --- | --- | --- |
| Every folded hash, in chain order (~35 B per entry) | `Snapshot::folded`; `WriterLog::chain`, `next`, `branches`; `FoldHashes` in the model | Placing an entry after any folded one; a fork from any point (`Anchors.cfg`); telling a re-delivered line from a new one (`holds`); a snapshot superseding another (`extends`); `Doomed` deleting only folded lines (`FoldedOnly`); writer continuation (genesis and recorded head held, `writer.rs`); a pending record's standing (`Chain::holds`, `continues`) | Replaced by sequence numbers, a window of K hashes, and certificates of files (sections 1, 4) |
| Every register write, replaced ones too, and a grow-only `replaced` set | `merge::Register` | A snapshot never brings back a write another log replaced; undo writes back the replaced value (`register_write`); adopt (`beyond`) | Own live write plus references until acknowledged (sections 2, 3) |
| Each removed set tag with its earliest remove | `OrSet::removed` | An add arriving after its remove stays removed; undo of an add (`removal`) | References until acknowledged; undo from session memory |
| A delete's `observed` list and a deleted entity's whole state | `Existence::Deleted`, `EntityState` | Deletion conflicts; reviving an entity whole | Owner's choice (open question 2) |
| Every trash item ever displaced, even after emptying | `Folded::trash`; `trash::list` filters by files present | Listing the trash with ages and origins | Dropped from the contribution once emptied |
| Settlements, unknown and extended entries | `Folded::settled`, `unknown`, `extended` | Not settling a record twice; newer builds' data | Kept (bounded by events and by mixed-build use) |
| Every entry ever held back, every fork ever reported | `WriterLog::strays`, `forks` | A fork with a held entry found after its file is gone; reporting each fork once | Reported forks kept; strays only while their files are visible |
| The cached view only grows; restored-away entries kept and reported | `CachedView`, `Reader::removed`, `let-go.json`, `Library::let_go`, `adopt` | `Monotone`, `ForksKept`; the restore choice | Dropped after a grace period (section 7) |
| Own intents since the last snapshot; `reverses` | `undo::History::of` | Undo, redo, history display | This session only, from memory (section 8) |
| Live tags of a re-added member | `intent.rs`: `FactChange::Add` makes an add with no `replaces` | Nothing: re-adding a present value adds another live tag, so live state grows with activity | `add` replaces the tags of its value it saw |
| Crash leftovers and retired writers | "What stays" in SPEC.md | `SealedOnly`; a writer that never runs again | Unchanged; bounded by events, not by history |

## 1. Sequence numbers and references

Every entry gains `n`: 0 for the genesis entry, its predecessor's plus one
after. A reader does not place an entry whose `n` is not its predecessor's plus
one, and reports it.

Every reference to another entry's write, add or existence write — in
`replaces`, `observed`, `tags`, and the new `base` — becomes
`[writer, n, hash]`. The writer and number let a reader judge whether it has
folded the target without remembering its hash; the hash still identifies it
exactly, so a fork's entries collide on nothing.

`add` gains `replaces`: the tags of its value the writer saw. A member then has at
most one live tag per concurrent adder.

Every op on an entity other than `create` gains `base`: the existence writes of
the entity it saw survive. Entries that create the entity omit it. `base` is read
only on arrival (section 5) and never kept in a snapshot.

`reverses` is dropped (section 8).

## 2. Snapshots hold a contribution, not a history

A snapshot holds what its writer still contributes to the merged state, judged
by everything the writer had read when it compacted:

| Member | Value |
| --- | --- |
| `writer`, `label`, `at` | As now |
| `genesis` | The hash of the genesis entry, so continuation and the label need no chain |
| `n`, `head` | The number and hash of the last folded entry |
| `window` | The hashes of the last K folded entries, ending at `head` |
| `certified` | For each file in the directory the compaction leaves or deletes, and each file the last C compactions deleted: for a segment, its first hash and the hash and position of the last of its lines the snapshot folds; for a snapshot, its `head` |
| `frontier` | For every writer the compacting writer had read: `[writer, n, hash]` of the last entry of that writer's chain it held |
| `state` | The contribution, below |

The contribution, per entity:

- **Registers, the file register and existence.** The writer's own write that
  survives in its view, if any: at most one per register, because each own write
  replaces every survivor it sees, its own earlier write included.
- **Sets.** Per member, the writer's own add that survives in its view, if any:
  at most one, now that `add` replaces the tags it saw.
- **References.** `[writer, n, hash]` of other writers' writes, adds and
  existence writes that its entries replaced, removed or observed, until each is
  acknowledged.

Plus the writer's trash items not yet emptied, settlements and dismissals
(section 5), and its own unknown and extended entries.

A write that holds no value — a clear, a delete, or a removal carried only as
references — is dropped once all of its references are acknowledged: nothing is
left for it to hide.

A reference `[y, n, h]` is **acknowledged** when the latest snapshot of `y` that
the compacting writer accepted covers it and does not hold it: that snapshot's
head is at `n` or later on the same branch (`h` in its window, or `n` below its
window while `y` shows no fork or forgotten-basis run), and `h` is not among its
live writes. `y` then holds the target dead itself, so the reference has nothing
left to kill in any reader that has that snapshot.

A retired writer never compacts again, so a reference to one of its live writes
is never acknowledged. That costs at most one reference per field per retired
writer, bounded by the state, not by history.

### Why contributions, and why references stay explicit

Two smaller designs fail:

- **Bounded hash tail, unchanged merge.** Memory stays linear: registers and sets
  keep their tombstones.
- **Causal contexts (a version vector per write) in place of references.** A
  context component names a writer's position, not which branch of a fork it is
  on. Below the window a reader cannot tell the clone's entry 9 from the
  original's, and a context naming the clone's would silently replace the
  original's writes 6–9. An explicit hash never kills the wrong write; the worst a
  wrong judgment can do is drop a reference early, which keeps a write alive as a
  visible conflict.

A third was considered and set aside: **every snapshot holds the writer's full
merged view** with a version-vector context. Joins of full states need no
tombstones and never regress, but every writer's directory then holds a copy of
the whole library, retired ones included; a facts copy outlives the writer whose
entries it came from, so restored-away facts come back in another writer's
snapshot; and fork ambiguity spreads into every snapshot's context instead of
staying in one directory.

## 3. The merge

A reader's merged state is the join, over writers, of each writer's
contribution — its accepted snapshot plus the entries placed after the
snapshot's head. A write or add is **live** while it is in some contribution and
no reference in any contribution or placed entry names it.

Per field, the reader keeps the live writes and adds with their numbers, hashes,
clock readings and values, and **pending references**: references whose target
it has not placed. A pending reference is dropped once the target's writer's
frontier in the reader passes the target's number without the target being live.

| Type | A contribution keeps | An arriving write `w` is compared with |
| --- | --- | --- |
| Register, file register | Own live write (≤ 1); references until acknowledged | Pending references naming `w`: if one does, `w` arrives dead. Each reference of `w`: a live target is killed; an absent target whose number the reader's frontier has passed is already dead, and the reference is dropped; any other becomes pending |
| Set | Per member, own live add (≤ 1); removed tags of others until acknowledged | As a register, per member: an add's `replaces` and a remove's `tags` are references |
| Existence | Own live create or delete (≤ 1); references; see open question 2 for observed field writes | As a register. A live delete together with a live field write it did not observe is a deletion conflict, as now |
| Arrival of any op | — | Its `base` (section 5) |

There is no ordered-list type today. One would need position anchors that
outlive their elements and its own retention rule.

### Replacing a contribution

When a newer snapshot of `x` is accepted, the reader replaces `x`'s
contribution. What the newer snapshot adds takes effect at once. What it dropped
— `x`'s writes it no longer holds, references it acknowledged — is dropped only
once the reader's frontier **dominates** the snapshot's `frontier`: for every
writer, the reader holds that writer's chain at least as far, on the same branch.
Until then those writes and references stay as they were.

This is what keeps a cached reader monotone: a snapshot drops `x`'s write because
`x` saw another writer's write replace it, and a reader that has not yet read that
other write would otherwise show the field empty in between. A reader without a
cached view has nothing to defer and may show that gap while files arrive, as it
may today (`FreshMonotone` is already false).

## 4. Placing entries

The reader remembers, per writer `y`:

- the accepted snapshots, as contents rather than files;
- the entries in flight: placed, folded by no accepted snapshot, and held by no
  file it sees, as when a segment's deletion arrives before the snapshot that
  folds it;
- the reported forks and the file records, as now.

Everything else it knows of `y` comes from the files it sees. When the accepted
snapshots change, it places the entries of those files again from what the
snapshots let it chain from and what it remembers. Placement is then a function
of `y`'s files once they have all arrived, so every reader agrees on it at rest.
(Remembering every placed entry instead would let a reader that saw a fork early
keep merging a branch that a reader without its memory shows as a conflict.)

**Main snapshot.** Among the snapshots of `y` the reader holds or sees, the one
whose head is latest by clock reading, then number, then hash. A snapshot is
accepted when it is the main one, or its head is in the main one's window or
certificates, or its head is a placed entry. A snapshot that is not accepted has
a forgotten basis (section 5); if it was accepted before, the reader stops
merging it.

Let *base* be the main snapshot's `n`. An entry `e` with hash `h`, predecessor
`p` and number `n` is:

1. **Folded** when `h` is in the window or certificates, or `e` lies at or
   before the certified line of a file whose first hash and that line match a
   certificate: skipped. Each line is the predecessor of the next, so certifying
   a line certifies every line of the file before it. This is how a re-delivered
   file, a late deletion, a crash leftover, or the folded part of a segment that
   runs on past the head is recognized.
2. **Placed** when `p` is placed, in the window, or certified (or `e` is a
   genesis entry), and `e` is within reach: `n > base − K`, or `y` has no
   accepted snapshot. When another placed entry has predecessor `p`, it is a
   fork: both are merged and the fork is reported once.
3. **A gap** when `p` is not held and `n − 1 > base − K`: `p` is after the
   window's bottom, on the main chain or on a branch not yet seen. Held back and
   reported, as now.
4. **Forgotten basis** otherwise: `e` is out of reach, or `p` is not held and at
   or below the window's bottom, or `p` itself has a forgotten basis.

Within reach is what stops a re-delivered entry from being merged twice. Without
it, a genesis entry, or an entry after a certified line, that a snapshot folds
out of its window would be placed again on top of the snapshot, and in a
contribution model that can bring back what the entry replaced.

A fork is merged only while the folder lets a reader place its branch. Once the
original has compacted past the fork point without having seen the clone (it
retires as soon as it does), the clone's branch has a forgotten basis for every
reader, including those that merged it before.

## 5. Forgotten basis (D14)

**When it fires.**

- A run of `y`'s entries, or a snapshot of `y`, classified as forgotten basis.
  Causes: a copied local root that wrote after the original had moved more than K
  entries on and compacted (a fork from beyond the window); a sync client
  delivering a file deleted longer ago than C compactions; a virtual machine
  resumed twice (already outside the model).
- A placed op whose `base` names existence writes that the reader's frontier has
  passed, while the reader holds no state for the entity: an edit, made by an
  instance that had not read the deletion, to an entity whose deletion has been
  acknowledged and forgotten (open question 2).

A run whose predecessor has merely not arrived is a gap, never a conflict.

**What the user sees.** One item per run: the writer's label, the clock range of
its entries and how many there are, captioned as changes made to a version of the
library this install no longer has. Inside it, each op that disagrees with the
current state, as a conflict of its kind:

| Op in the run | Shown when the current state | As |
| --- | --- | --- |
| Register or file write of `v` | Shows other values | Current values against `v` |
| Add of `v` | Lacks `v` | `v` added |
| Remove of `v` | Has `v` | `v` removed |
| Create, or an op on a forgotten entity | Does not show the entity | The entity, with the run's values for it |
| Delete | Shows the entity | The entity deleted |

Ops that agree with the current state are not shown, so a re-delivered old file
shows only what has changed since. Nothing in the run is merged until the user
chooses. Settle entries in the run are applied, since a settlement is a fact
about a record that is true whoever logged it. A pending record whose `prev` is
the last entry of a run is reported as an orphan, as now.

**Choosing.** Keep current or take theirs, per op or for the whole run. The
choice commits one intent: each op taken becomes a new op against the current
state (a write replacing the current survivors, an add, a remove, a create), and
a `dismiss` op names the run by `[writer, first hash, last hash]` or the
snapshot's head. No reader that has the dismissal shows the run again. A
dismissal stays in its writer's contribution while the run's files remain,
which for a retired writer is forever; one per event.

A writer whose own directory holds a forgotten-basis run or a fork retires, as
it retires on a fork today.

## 6. Compaction

A writer folds its contribution as in section 2, from what the folder holds as
it reads it now. It does not compact while its view holds an entry no visible
file holds (section 7), so a snapshot never publishes restored-away knowledge.

It then deletes, as now only after the folder holds the new snapshot and each
file is checked just before it goes:

- each segment that ends with a seal marker and that it certified: every line
  was placed on its own chain at or before the head;
- each snapshot whose head is in the new snapshot's window or certificates.

It never deletes a segment without a seal marker (`SealedOnly`) or a file it
could not place on its chain (`FoldedOnly`). Such a segment stays certified in
every later snapshot: one certificate of about 40 bytes per crash or copy.

Superseded files may go at once. D12 means a restore need not be survived, and
no two-phase deletion is needed, as before. `CheckFirst` stays: it is what keeps
a writer from appending after an entry the folder lost.

A writer compacts at close and whenever its own entries since its last snapshot
reach K. That bounds what every reader holds whole for it.

## 7. The cached view and D12

The view keeps, per writer, the accepted snapshot contents and the entries
placed after them; the merged state with pending references and deferred drops;
the file records; the reported forks; the forgotten-basis runs and dismissals.
It no longer keeps every hash, strays of files no longer visible, restored-away
entries past the grace period, or `let-go.json`.

**Restored-away facts.** An entry or snapshot the view holds that no file in the
folder holds can mean a restore, or a deletion that arrived before the snapshot
replacing it; a reader cannot tell them apart. The reader keeps showing it,
without a report, until its absence has lasted the grace period across at least
one full rescan. It then derives the whole merged state again from the folder's
files. Facts only the restore took then disappear, with no prompt.

A sync client slower than the grace period makes those facts disappear, and the
entries after them wait as a gap, until the replacing snapshot arrives. Edits
made in between are judged against the smaller view, so at worst they produce
visible conflicts.

`Library::let_go`, `adopt`, `Reader::removed`, `Opened.removed` and
`Refreshed.removed` go.

## 8. Undo (D11) and drafts (D13)

The undo stack lives in memory and holds this session's intents only. Each item
keeps what reversing it needs: the values its writes replaced, the tags its
removes named, its file effects. Refusals still compare against the current
merged state ("changed since"). The stack ends at close, and `reverses` is no
longer logged. Drafts are unchanged.

## 9. Writers and pending records

A writer continues only if its recorded head is its tip or in its window and the
snapshot's `genesis` is its genesis; otherwise it retires, which is always safe.
An open pending record's `prev` is its writer's last entry, so it is always held.
A record whose `prev` is held and followed is spent or closed, as now. One whose
`prev` is neither held nor in a forgotten-basis run is ignored and reported, as
now for a `prev` not in the log.

## What changes for the guarantees

**Kept.** Readers holding the same files compute the same state once delivery
completes and, after a restore, the grace period has passed. A kept entry is merged or shown, never silently lost. A replaced value
or removed member never comes back silently: a reference is dropped only after
the target's own writer holds it dead. Chain order; single-writer directories;
no bytes leave the folder except by emptying a trash; recovery is total;
opening writes nothing.

**Weakened, and why the decisions allow it.**

| Was | Now | Allowed by |
| --- | --- | --- |
| A fork from any point is merged and reported | Merged and reported while the folder lets a reader place its branch: within K entries of the main snapshot's head, or after a certified line. Otherwise shown as a forgotten-basis conflict, by every reader, including one that merged it before | D14 |
| A re-delivered file is always recognized | Recognized within the window or the certificates of the last C compactions; beyond, its changed ops are shown as a conflict | D14 |
| `Monotone` (I10): a reader never loses an entry unless its local root is lost | A reader stops merging an entry only when no file it sees holds it (after the grace period), or when it now holds it back or shows it as a forgotten basis | D12, D14 |
| `ForksKept` | A reported fork is never reported again; a fork among dropped entries is no longer shown | D12 |
| Reviving a deleted entity brings back every field | Depends on open question 2 | D10 |
| Undo reaches back to the last snapshot, and history is shown | This session only | D11 |
| Let go or adopt after a restore | The folder wins after the grace period | D12 |

**Dropped.** `FoldHashes`: a snapshot no longer lists every folded hash.

## The model after the change

A prototype of this section is in [spec/Portable.tla](spec/Portable.tla).

**Rules.** `FoldHashes` becomes `Window`, a number: how many of a chain's last
entries a snapshot lets a reader chain from. A window of at least `MaxEntries`
behaves as folded hashes did. New rule constants name what the new must-fail
configs drop:

| Constant | Rule |
| --- | --- |
| `Certify` | A snapshot certifies the lines it folds of every file of its directory, and a writer deletes only what a snapshot certifies |
| `Numbered` | A reader places an entry only within reach of the main snapshot's window |
| `Classify` | A reader shows an entry whose predecessor it forgot as a conflict instead of holding it back |
| `Grace` | A reader may derive its view again from the files it sees, at any time some merged entry is held by none of them |

**Shape.** Entries carry `n`. A snapshot keeps `folds` only as a ghost for
`Retained`; a reader may use only its `win` and `certs`. A reader remembers the
snapshots it accepted and the entries in flight, places the entries of the files
it sees afresh at every read, and records what it shows as forgotten (`shown`)
and what it holds back (`gaps`). `acc` stays as a ghost of what its merged state
reflects. The merge rules — references, acknowledgment, deferred drops — are
not modeled: `Portable.tla` abstracts the merge to a set of entries.

**Properties.**

| Property | After |
| --- | --- |
| `ChainOrder`, `OwnDirectory`, `Retained`, `EffectAccounted`, `DeliveredSettles` | Unchanged |
| `NothingIgnored` | Every entry a reader sees is merged, held back, or shown as forgotten |
| `NoReapply` (new) | A reader never places an entry from a file while a snapshot it accepted folds it |
| `NoSpuriousConflict` (new) | An entry is shown as forgotten only when its chain forked at or before it, when every file that shows it to the reader has been deleted, or while the reader remembers a snapshot a restore took |
| `DeliveredConverges` | Once every file has reached a reader, it merges or shows every kept entry, holds back only what a restore took, and reports every fork among the kept entries it merged; a reader without a cached view merges or shows the same |
| `Monotone` | Restated: a reader stops merging an entry only when no file it sees holds it, or when it now holds it back or shows it as forgotten |
| `ForksKept` | The reported list only grows |
| `Convergence` | Restated with the new `Converged` |
| `FreshMonotone` | Still false |

**Configs and results.** Each was run with TLC 2.19 by hand; the fast set
through `nix build .#nord.toshokan-spec`.

| Config | Bounds | Result |
| --- | --- | --- |
| `Portable` | window 1, 3 entries, 3 files, a clone | Passes: 309k states, 36 s |
| `PortableWide` (new, deep) | `Portable` with 4 files | Passes: 5.7M states, 26 min |
| `Recovery` | window 1 | Passes: 454k states, 42 s |
| `Grace` (new) | window 1, a restore, the grace step | Passes: 395k states, 40 s |
| `Liveness` | window 1 | Passes: 618k states, 3 min |
| `Names`, `Sealed`, `Folded` | — | Violate `Retained`, as before |
| `Unchecked` | — | Violates `DeliveredConverges`, as before |
| `FreshView` | — | Violates `FreshMonotone`, as before |
| `Anchors` | without `Classify` | Violates `DeliveredConverges`: a clone made before the first write writes a genesis entry the original's snapshot has folded out of reach, and the reader holds it back forever |
| `Uncertified` (new) | without `Certify` | Violates `NoSpuriousConflict`: a writer cannot delete a sealed segment whose lines fall below its snapshot's window, and a reader shows those folded lines as forgotten |
| `Unnumbered` (new) | without `Numbered` | Violates `NoReapply`: a segment deleted two compactions ago arrives late, and its genesis entry is placed on top of a snapshot that folds it |

`Portable` with the grace step on as well also passes at 4 files (6.2M states,
30 min). The fast set takes about 6 minutes, up from 2: `Liveness` alone takes
3, because TLC evaluates the read again for every fairness check.

**Found by the model while prototyping**, and folded into sections 2 and 4:

- Reach measured from the reader's own tip stopped it from placing again the
  genesis entry of a segment it still saw. Reach only matters against what a
  snapshot folds, so it is measured from the main snapshot's head.
- A clone's snapshot can sit in the middle of the original's open segment, so a
  certificate must cover the folded prefix of a file, not only whole files.
- When a higher-ranked snapshot becomes main, a reader must stop merging the one
  it replaced and show it as forgotten.
- A reader that remembers every placed entry keeps merging a fork branch that a
  reader without its memory shows as a conflict; it must place the files it sees
  afresh.
- After the grace step, entries after a snapshot the reader dropped wait as a
  gap, which `Monotone` must allow.

**Not yet checked.** The deep set. A liveness property that the folder wins,
which needs fairness on deletions reaching readers. Clock readings: the model
ranks snapshots by number, then hash.

## Expected size

Steady state, the harness's schema, no conflicts. An entity holds about 8.7 live
writes: a create, 2.7 registers (`name`, `origin`, two thirds have `rating`),
3 tags, 1 related, 1 file.

**Memory.** A live write in memory is its hash (16 B), number (8), clock and
writer (32), value handle (~24) and map overhead (~16): about 100 B, so about
0.9 KB per entity, plus the entity's maps (~0.25 KB) and the view's index
(measured: 0.4 KB). At minimal history the current code holds 2.3 KB per entity
(4.5 MB open cold at 400 entries), which is the figure this design should
reach. Per writer add the window (K × ~40 B = 40 KB at K = 1024) and the entries
since its last snapshot (up to K entries at ~400 B = 0.4 MB).

2,000 entities, 3 writers: 2.3 KB × 2,000 + 3 × (0.04 + 0.4) MB ≈ 6 MB, at
any history length.

**Folder.** A contribution holds each live write once, in its writer's
snapshot, at about 110 B of JSON each (hash 34, `n` 8, clock 18, value and keys);
the writer's id is the snapshot's, not repeated. That is about 1.0 KB per entity
plus ~0.1 KB of entity and key overhead. Per writer add the window (1024 × 32 hex
digits = 32 KB), certificates (~40 B each), the segments since its last snapshot
(≤ K lines, ~0.4 MB), and its crash leftovers.

2,000 entities, 3 writers: 1.1 KB × 2,000 + 3 × 0.45 MB ≈ 3.6 MB.

**Cached view.** The merged state packed (~1 KB per entity, the current measured
figure) and the contributions (~0.7 KB per entity): ≈ 3.5 MB.

A retired writer adds its final contribution and the references others keep to
it, at most the size of what it wrote: bounded by writers × state.

## An old folder opened by the new build

There are no migrations before 1.0. The new layout reads `writers.2/` instead of
`writers/`, so an older build and this one never read each other's lines: an old
folder opens as an empty library whose files are all unbound, and the old
directories are left for the older build. The cached view's format byte changes,
so old views are discarded and the folder is read from scratch. `let-go.json`
and old `head.json` files are ignored; the first write starts a new writer.

## Risks and open questions

1. **K and C.** K = 1024 and C = 4 are guesses. A larger K merges clones that
   forked further back and recognizes older re-deliveries, at 40 B of memory and
   32 B of folder per writer per hash.
2. **Deleted entities.** Either (a) a deleted entity's fields stay live, so a
   deletion conflict revives it whole and the entity is kept forever (about 1 KB
   per entity ever deleted); or (b) a delete kills the field writes it observed,
   so a deletion conflict revives only the concurrent edit and the entity
   disappears once acknowledged; a later edit made offline shows as a forgotten
   basis. (b) bounds the state. Recommended: (b).
3. **Grace period.** Its length, and whether a full rescan must fall inside it.
   Too short makes facts flicker under a slow sync client; too long keeps
   restored-away facts on screen. Suggested: 24 hours and one full rescan.
4. **Which branch of an old fork is main.** The latest clock reading wins; the
   other branch's changes are shown as conflicts, even when the other branch is
   the original. Deterministic, but surprising.
5. **False conflicts after a resurrection.** A sync client that brings back a
   file deleted more than C compactions ago makes its changed ops appear as
   conflicts until dismissed.
6. **A dropped reference resurrects as a conflict.** If a reference is
   acknowledged against the wrong branch of a forked writer, or a restore takes
   only the target writer's newer snapshot, the target comes back live beside the
   replacing write: visible, never silent.
7. **Deferred drops can wait forever.** If entries a snapshot's frontier names
   were restored away, a cached reader never dominates it; the grace-period
   re-derivation is what ends that.
8. **`base` costs bytes in segments** (~75 B per op) to detect edits to
   forgotten entities. With option (a) of question 2 it is unnecessary.
9. **A virtual machine resumed twice** writes two branches into one segment.
   Today readers report a fork. Here a diverging copy fails its certificate
   past the point where it diverges, and its later lines are placed as a fork
   when that point is within reach, or shown as forgotten when it is not. It
   stays outside the model.
10. **The merge rules are unmodeled.** Acknowledgment and deferred drops are the
    newest reasoning here and the most likely place for an error that resurrects
    data; they need exhaustive tests before code depends on them.
11. **Crash leftovers still accumulate**, one segment of up to 1 MiB per crash or
    copy, as today.
