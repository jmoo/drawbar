# Bounded retention

This design bounds how much history toshokan keeps. [SPEC.md](SPEC.md)
specifies the format and protocol it led to; this document keeps the reasoning,
the guarantees and what the model and the value-level checks found. The model in
[spec/](spec/) follows it, and `tests/retention` checks toshokan against it.

## Decisions

- **D10.** Keep only as much history as stops two instances stomping on each
  other. History is not for version-control features.
- **D11.** Undo and redo reach only this session's intents and end at close.
- **D12.** After a folder is restored from an older copy, the folder wins: no
  retained view of restored-away facts, no let go or adopt.
- **D13.** Unsaved edits stay local to each install.
- **D14.** A write whose basis was forgotten is shown as a conflict, never
  resolved silently either way. There is no offline time horizon.

Settled for this design: a delete kills the fields it observed, so a revive in
a deletion conflict brings back only the concurrent edit; the grace period is 24
hours and at least one full rescan; the new format lives under `writers.2/`.

## The problem

At a constant 2,000 entities, cost grows linearly with history. With
`examples/scale.rs --entities 2000 --files 2000 --writers 3`:

| Entries | Memory (open cold) | Folder | Cached view |
| ------: | -----------------: | -----: | ----------: |
|     400 |             4.5 MB | 1.8 MB |      2.0 MB |
|      2M |             767 MB | 505 MB |      286 MB |

A snapshot keeps the hash of every entry it folds, and every register keeps every
write it ever replaced, so compaction saves about a quarter and no memory.

## What history protects today, and what replaces it

| History kept today | What it protects | Replaced by |
| --- | --- | --- |
| Every folded hash, in chain order (`Snapshot::folded`, `WriterLog::chain`) | Placing an entry after any folded one; a fork from any point; telling a re-delivered line from a new one; `FoldedOnly`; writer continuation; a pending record's standing | Sequence numbers, certificates of every file the chain was written to, and fork points (sections 1, 2, 4) |
| Every register write and a grow-only replaced set (`merge::Register`) | A snapshot never brings back a replaced write; undo's previous value | Own live writes and, per field, one reference per other writer (sections 2, 3); undo from session memory |
| Each removed set tag with its earliest remove (`OrSet::removed`) | An add arriving after its remove stays removed; undo of an add | The same references, per member |
| A delete's observed list and the deleted entity's whole state | Deletion conflicts; reviving whole | A delete that kills what it observed and keeps its references |
| Every trash item ever displaced, even after emptying (`Folded::trash`) | Listing the trash | Items leave the contribution once emptied |
| A new live tag for every add of a present value (`intent.rs`) | Nothing | `add` replaces the tags of its value it saw |
| Entries ever held back, forks ever reported | Finding a fork after its file is gone; reporting once | Fork points, remembered until the grace step |
| Restored-away entries in the cached view, `let-go.json`, let go and adopt | `Monotone`; the restore choice | The grace step (section 7) |
| Own intents since the last snapshot, `reverses` | Undo, redo, history display | This session's undo stack, in memory |

## 1. Entries

Every entry gains `n`: 0 for the genesis entry, its predecessor's plus one after.
A reader does not place an entry whose `n` is not its predecessor's plus one, and
reports it.

Every reference — in `replaces`, `observed` and `tags` — becomes
`[writer, n, hash]`. `add` gains `replaces`: the tags of its value it saw, its own
included, so a member has at most one live tag per concurrent adder.

A `dismiss` op names runs the user dismissed, and a `take` names the run whose
change an op takes (section 5). `reverses` is dropped. Each entry also records
the fork points its writer knew of, by writer and number.

## 2. Snapshots

A snapshot holds what its writer still contributes, plus what lets readers
recognize every entry it folds:

| Member | Value |
| --- | --- |
| `writer`, `label`, `at` | As now |
| `genesis` | The hash of the genesis entry |
| `n`, `head` | The number and hash of the last folded entry |
| `certified` | Certificates, below |
| `state` | The contribution, below |

**Certificates.** A snapshot certifies every entry it folds, so that whenever a
file holding one reaches a reader, under any name, the reader skips it:

- by a segment certificate, where its writer knows a segment its chain was
  written to: the segment's first hash, and the hash and position of the last of
  its lines the snapshot folds;
- by the head of every snapshot of its directory on its chain;
- by hash, every other folded entry: one no segment it knows of holds, as after
  a sync client dropped a file's newest version;
- by hash, the last entry each of its dismissals names on its chain, so a reader
  can tell which branch a dismissal meant after the segment is gone.

The by-hash rule is what makes certification complete; segment certificates only
keep it small. Its writer knows segments from its own last snapshot (whose
contents its cached view holds even after another instance of the same writer
deleted the file), from the files it read or wrote since, and from the files in
its directory now. A view that lags costs hashes, not correctness. Certificates
are kept forever.

**Matching a file to a segment certificate.** A file matches when its first line
has the certificate's first hash and its line at the certified position has the
certified hash; its lines up to that position are folded. An unsealed file that
starts with the first hash but is shorter waits: its lines are held back, and
are not evidence of a fork, since sync may still be bringing the rest. After the
grace step its lines past the first are held back as items the user can take
from: the reader cannot tell a version a sync client dropped from a copy that
diverged before its end. A sealed file cannot still be arriving, so a sealed
file shorter than the certificate diverged from it, as does a file whose line at
that position has another hash, as copies of a virtual machine resumed twice do:
only its first line is folded, and the rest is placed or shown as a run by the
rules below.

**Contribution**, per entity:

- **Registers, the file register, existence.** The writer's latest own write to
  each, live or not: whether it survives is decided at merge time, by references,
  as for every other write. With it, the number of the writer's first own write
  to it, which tells a forgotten basis from none (section 4).
- **Sets.** Per member, the writer's latest own add or remove, live or not, and
  the number of its first.
- **References.** Per register and per set member, and for each other writer,
  the latest `[own n, own hash, writer, n, hash]` at or below the fork point of
  that writer its own entry knew of, and the latest above it: the target the
  writer's entries replaced or removed, and its own entry that did, carried
  forward across its own later writes. A delete is a reference on every field it
  observed: each write it observed is a target on that write's field. -
  **Takes.** Per field and other writer, the latest take of a run of that
  writer: its own entry and the run's first number. - **Own deletes.** Per own
  field write it holds, the first own delete above it, `[n, hash]`: that delete
  kills the write, and every earlier own write of the entity.

Plus trash items not yet emptied, settlements, dismissals, and its own unknown
and extended entries. Nothing is dropped for being dead, and references are never
dropped; a contribution holds at most one own write, one number and one own
delete per field, and per field and other writer one take and two references,
one on each side of a fork point it knows of.

## 3. The merge

A reader's merged state is the join of every writer's contribution: its tip
snapshot and the entries placed after it. A write or add is **live** while its
writer's contribution holds it and nothing kills it:

- **Own writes.** A writer's latest own write on a field kills that writer's own
  writes numbered below it on that field. A reader that held an older own write
  and receives a newer tip therefore drops it at once.
- **Own deletes.** An own delete a contribution holds kills that writer's own
  field writes of the entity numbered below it.
- **References.** A reference `[own n, own hash, y, n, hash]` kills, on its
  field, every write of `y` numbered below `n` and the one numbered `n` with
  that hash. One numbered `n` with another hash is on another branch, and is not
  killed. A reference applies only when the writer holding it is not forked, or
  its `own n` is at or below that writer's fork point: references made on a
  branch are part of the run. A reference whose target is numbered above `y`'s
  fork point kills only the write with its hash: `y`'s earlier writes it passed
  may have died only on that branch.
- **Takes.** A take of a run of `y` kills, on its field, every write of `y`
  numbered below the run's first entry: the user chose the run's value over
  what the branches started from, which a reader may no longer hold.

Killing below `n` is sound because `y`'s write `n` replaced every earlier write of
`y` to that field it saw, and `y` always sees its own; and no two branches of a
writer are ever merged (section 4), so every merged write of `y` lies on one
chain.

Because references are kept per other writer, one made before a fork its writer
had not seen can be overwritten by a later one made after it. Such a reference
no longer applies, and the write it killed comes back beside the writer's value
as a conflict: shown, never lost. A writer that has seen the fork keeps one
reference on each side of it, so a take, which names both the merged write and
the run's, keeps both.

| Type | A contribution keeps | An arriving write or add is |
| --- | --- | --- |
| Register, file register | Latest own write; per other writer, the latest reference | Dead if an own later write or a reference covers it; it kills what its references cover |
| Set member | Latest own add or remove; per other writer, the latest removed tag | As a register, per member: an add's `replaces` and a remove's `tags` are references |
| Existence | Latest own create or delete; per other writer, the latest reference | As a register. A live delete beside a live field write is a deletion conflict |

**Deletes.** A delete kills the entity's existence writes and field writes it
observed. A field write it did not observe stays live, so a concurrent edit
leaves the entity shown in a deletion conflict; reviving it brings back only that
edit. Its references are kept forever, so an edit made long after, by an
instance that had not read the delete, is still a deletion conflict. A delete
kills below `n` only on the field of the write it observed: another field's
earlier write of that writer may have been dead only because of a write a
restore later took.

**A newer snapshot of a writer.** A tip that certifies the previous tip's head
replaces that writer's contribution and the entries placed after the old tip that
it folds. The writer's latest own write on each field is in it, so an older own
write a reader held is killed by the own-write rule at once.

**A contradicted reference.** If, after the grace step, a reference names a
number above the highest the files show for its target writer (a restore of one
directory, or a dropped version), or a number at which the files show only
another entry of that writer (a restore that took one branch), every reference
of the same entry or contribution to that writer kills only the write with its
exact hash, and the reader reports it. What only the lost write replaced then
shows again, as it would for a reader holding every entry. Where a snapshot
holds the reference instead of its entry, the earlier exact targets are gone,
and a write the lost one did not replace can come back too: beside the current
value as a conflict, never in place of it. Readers that noticed the absence at
different times disagree on that field until both have passed the grace period.

## 4. Placing entries

Per writer `y`, from the files the reader sees and what it remembers:

- **Tips.** The snapshots of `y` no other snapshot of `y` certifies.
- **Folded.** An entry a tip certifies by hash or head, or a line of a file that
  matches a segment certificate (section 2). Skipped. This recognizes
  re-delivered files, late deletions, conflicted copies, crash leftovers,
  resurfaced files and the folded part of a segment that runs on past a tip's
  head.
- **Waiting.** The lines of a file that starts like a certified segment but is
  shorter. Held back until the rest arrives.
- **Placed.** An entry whose predecessor is a tip's head or placed, or a genesis
  entry when `y` has no tip.
- **Gap.** Any other entry whose predecessor may still come after the tips'
  heads: `n − 1` above the highest tip head. Not merged; shown from the start as
  an item the user can take changes from, not dismiss (section 5).
- **Run.** Any other entry, and every entry after one.

**Forks.** A fork point is an entry with two successors among those placed or
seen, an entry a tip certifies that is the predecessor of an entry no tip folds
(the tip's chain holds the other successor), or the predecessor of a line no tip
folds that is numbered at or below a tip's head: certificates are complete, so
that line is off the tip's chain, even when the predecessor sits inside a
segment certificate and cannot be checked by hash. Every entry of `y` numbered
above a fork point, on either branch, and every live write in a tip of `y`
numbered above it, is a run: no branch is merged beside another. A reader
remembers the fork points it found, so a fork stays a fork when one branch's
files go before the other's; certificates make every fork point derivable from
the files again, so a reader without that memory finds the same ones, except for
a fork inside a segment (open question 7).

**Several tips.** When two snapshots of `y` do not certify each other, the
writer forked and both sides compacted. Their parting is the highest entry every
tip names by hash anywhere: in its certificates, as its head, as an own write,
as the own entry of a reference, as an own delete, or as a dismissal. Two tips
that name one hash both have it on their chain, so the parting never lies above
the true fork point, and every entry that still has an effect on a tip names
itself there. A pre-fork entry with no effect left may lie above the parting; it
is shown as a run, which costs display only. Below the parting the reader merges
the union of the tips' own writes: a tip lacks a pre-fork write only because its
own later write replaced it, and that later write is above the fork point and a
run. Above it, every tip's own writes and references are runs.

Two tips with one head are copies of one chain compacted twice: their own writes
and references are the same, since both follow from the chain; the reader takes
the union of the rest (trash items, settlements).

**A forgotten basis.** A tip's own write the merge leaves out, because it is
above the fork point or a dismissal names it, may have replaced an earlier own
write the tip no longer holds. The tip's first-write number says whether it did:
it did when that number is below the write's, and, for a write above the fork
point, no higher than the fork point could be. Where several tips part inside a
segment, that is just below the lowest head. When it did and no merged write of
that writer on that field is numbered below it, the value is forgotten. The run
then shows that field whatever the merged state shows, and, like a gap, the user
can take it but a dismissal does not hide it; only a take of a run of that
writer on that field settles it. A write that merely names the run's write, by a
writer that had not seen the fork, does not. This is D14's conflict for a
forgotten basis. It arises whenever a branch compacts past a pre-fork write it
replaced, so a branch's compaction can change the merged value of such a field.
An entity left with no live existence write this way shows its live fields, as
in a deletion conflict.

**What the reader remembers** of `y`: its tips, the entries it placed after
them, the entries it shows as runs, its fork points and its file records. It
places the entries of the files it sees, and those it remembers, again from the
tips' heads whenever its tips change, so its placement is a function of the
files once they have all arrived, and readers agree.

The window of recent hashes is gone. Re-deliveries are recognized by
certificates, and a fork near the head is found by the two successors of its
fork point; a window would only let a reader merge a recent fork, which runs
forbid.

## 5. Runs (D14)

**When a run appears.** A writer's history forked: a copied local root, a
virtual machine resumed twice, a restored backup of a local root.

**Gaps.** An entry held back after a gap is an item from the start: the user can
take its changes, as ordinary writes, but cannot dismiss it, and nothing about it
expires. When the gap fills, the entries are merged; a change the user took
writes the same value, so the field shows one value.

A resurfaced or late file of an unforked writer is never a run: certificates
recognize it.

**Identity.** A run is named by its writer and its first entry's hash, with the
branch it lies on; a tip run by the tip's head. Readers that see the same files
agree on both.

**What the user sees.** One item per run: the writer's label, the clock range of
its entries and how many there are. Inside it, the net change the run makes:
the run's entries folded alone, then compared with the current state, field by
field. A run that creates an entity, edits it and deletes it shows nothing. A
register shows the current values against the run's; a set shows members the
run added or removed; existence shows an entity the run created or deleted.

**Choosing.** Keep current, or take the run's change, per field or for the whole
run. The choice commits one intent: each change taken becomes an ordinary op
against the current state that also names the run in `take`, and a `dismiss` op
names the run as `[writer, first hash, last hash]`, or a tip run by its head. A
dismissal by a tip's head also covers the writes numbered up to that head in any
later tip that certifies it. No reader shows or merges what a dismissal names,
ever, even if it later stops seeing the fork: a tip's references and own delete
made by a dismissed entry do not apply either. An entry after a dismissed one is
a new item, never placed. A dismissal never covers what the user has not seen. A
tip matches a dismissal by the hash of its last entry, which every snapshot on
that chain certifies (section 2).

A branch that keeps writing until its instance sees the fork produces a new item
with each sync.

**Retirement.** A writer takes a new id only when it sees an entry of its own
directory with two successors. A gap in its directory does not retire it.

## 6. Compaction

A writer compacts at close, and when its own entries since its last snapshot
reach a threshold, whenever its contribution changed: new entries, own writes
newly dead, trash emptied. It folds from what the folder holds as it reads it
now. It deletes, only after the folder holds the new snapshot and each file is
checked just before it goes:

- each segment that ends with a seal marker, every line of which the new snapshot
  certifies on its own chain;
- each snapshot whose head the new snapshot certifies.

It never deletes a segment without a seal marker (`SealedOnly`) or a file it
could not place on its chain (`FoldedOnly`). Superseded files may go at once.
`CheckFirst` stays: a writer never appends after an entry the folder lost.

## 7. The cached view and restores (D12)

The view keeps, per writer, the tips' contents, the entries placed after them,
the entries shown as runs, the fork points, the file records, and the merged
state; and the dismissals. It no longer keeps every hash, `let-go.json`, or anything a restore
took once the grace period passes.

**The grace step.** When the reader holds something of a writer that no file it
sees holds — a tip whose file is gone, an entry in flight — it keeps showing it,
without a report. Once that has lasted 24 hours and at least one full rescan, it
drops each such item from which nothing it sees descends: no line whose chain of
predecessors reaches it, and no snapshot that certifies it. A greater number on
another branch is not enough, or a restore that took one branch of a clone would
leave that branch shown for good. It also forgets that writer's fork points the
files no longer show, as when a restore took one branch. It then places that
writer's files again. Facts only a restore took disappear, with no prompt.

The guard is what tells a restore from a slow sync. It cannot tell a snapshot
lost by hand, or by a sync fault, while a newer segment survives: the cached
reader keeps the snapshot, a fresh one holds the segment back as a gap, and they
disagree for good. A dropped version of a segment does the same to the entries it
lost while a later line of the same chain survives. Neither is something the
protocol does. After a compaction, a reader
may receive the deletions before the new snapshot; the writer's later entries,
which descend from the missing snapshot's head, show it has moved on, and the
reader keeps waiting. A writer that wrote nothing since compacting gives no such sign: after
24 hours its facts disappear until the snapshot arrives, and entries after them
wait as a gap. Edits made meanwhile are judged against the smaller view, so at
worst they produce visible conflicts.

`Library::let_go`, `adopt`, `Reader::removed`, `Opened.removed` and
`Refreshed.removed` go.

## 8. Writers, pending records, undo and drafts

A writer continues only if its recorded head is its tip or placed after it, and
the snapshot's `genesis` is its genesis; otherwise it retires.

An open pending record's `prev` is its writer's last entry, so it is always held.
A record whose `prev` is held and followed is spent or closed. One whose `prev` is
neither held nor in a run is ignored and reported, as now.

The undo stack lives in memory and holds this session's intents only. Each item
keeps what reversing it needs: the values its writes replaced, the tags its
removes named, its file effects. It ends at close. Drafts are unchanged.

## What users see

- The library as every writer's live writes say, with conflicts where writes are
  concurrent, as now.
- After a copied local root, VM resume or restored local-root backup: one item per
  branch of that writer's history after the fork, with the net change it makes,
  to keep or dismiss. Neither branch's later changes are merged until the user
  chooses; the writers involved retire and continue under new ids.
- After a folder restore: for up to a day, and a full rescan, the facts the
  restore took stay; then they disappear, without a prompt.
- A field another writer emptied, or an entity another writer deleted, while
  this one edited it: a register conflict, or the entity shown in a deletion
  conflict with only the concurrent edit.
- Entries after a gap: an item from the start, whose changes the user can take,
  merged as soon as the gap fills.
- A field whose earlier value a branch compacted away: shown in that branch's
  item until the user takes a value, even when it matches the merged one.
- Undo reaches this session only.

## Guarantees

**Kept.**

- Readers holding the same files merge and show the same entries once delivery
  completes and, after a restore, the grace period has passed.
- Every kept entry is merged, shown as a run, dismissed by the user, or held back
  after an entry a restore or a dropped version took.
- An entry no fork precedes and that follows nothing lost is merged.
- No two branches of a writer are ever merged together.
- A run stays shown until it is dismissed, its files go, or the grace step.
- A step by one branch of a forked writer changes no merged value, except on a
  field shown as a forgotten basis.
- A replaced value or removed member never comes back without a conflict:
  references are never dropped, and a write a reference covers stays dead.
- Chain order, single-writer directories, no bytes leaving the folder except by
  emptying a trash, total recovery, opening writing nothing.

**Given up**, and what allows it.

| Was | Now | Allowed by |
| --- | --- | --- |
| A fork's branches are merged side by side and reported | Each branch after the fork point is a run for the user | D14 |
| The cached view only grows | The grace step drops what no file holds and nothing visible descends from | D12 |
| A pre-fork value stays merged while the branches decide | A branch that compacts past it shows the field as a forgotten basis | D14 |
| Let go or adopt after a restore | The folder wins after the grace period | D12 |
| Reviving a deleted entity brings back every field | Only the concurrent edit | Ruling on D10 |
| Undo reaches back to the last snapshot; history is shown | This session only | D11 |
| A cached reader shows a field until its replacing write arrives | A fresh reader may show it empty meanwhile | D10 |

## Expected size

Steady state, the harness's schema: about 8.7 live writes per entity (a create,
2.7 registers, 3 tags, 1 related, 1 file).

| Part | Memory | Folder (JSON) |
| --- | --- | --- |
| Live writes | ~100 B each; with maps and the view's index, 2.3 KB per entity (the current figure at minimal history) | ~110 B each, ~1.1 KB per entity |
| References | ~48 B each | ~90 B each |
| A deleted entity | ~100–150 B, forever | the same |
| A removed member never re-added | one reference per remover, forever | the same |
| Certificates | ~60 B per segment ever written | ~90 B per segment ever written |
| Entries since the last snapshot | up to the compaction threshold, ~400 B each | ~250 B each |

References exist only where one writer replaced another's write: at most
(writers − 1) per field. The worst case for 2,000 entities and 3 writers is
2,000 × 8.7 × 2 × 90 B ≈ 3.1 MB in the folder; in practice far less.

For 2,000 entities, 3 writers and a compaction threshold of 1,024 entries, the
steady state is about 6 MB of memory and 4 MB of folder, plus:

- per deleted entity, ~150 B;
- per segment ever written, ~90 B: a segment holds up to 1 MiB, about 4,000
  entries, and every process that writes starts one, so 2M entries over years of
  daily use come to some thousands of segments, a few hundred KB;
- per retired writer, its final contribution and the references others keep to
  it, at most what it wrote.

These grow with deletions, segments and writers, not with entries: about three
orders of magnitude slower than today's 35 B of folded hash per entry.

## An old folder

There are no migrations before 1.0. This format lives under `writers.2/`; an old
folder opens as an empty library whose files are all unbound, and the old
directories are left for older builds. Old cached views are discarded by their
format byte; `let-go.json` and old `head.json` files are ignored.

## The model

[spec/Portable.tla](spec/Portable.tla) checks placement, forks, runs,
certificates, dismissals, the grace step, restores of the folder and of one
directory, and dropped versions, over an abstract merge (a reader's state is the
set of entries it merged). See [spec/README.md](spec/README.md) for the rules,
properties and configs.

Its rules are constants, so a config can drop one and show what it prevents:

| Without | What goes wrong |
| --- | --- |
| Certificates (`Uncertified`) | A writer cannot delete a segment below its snapshot's anchor, and readers show that segment's folded lines as a run |
| Carrying certificates forward (`Uncarried`) | A segment deleted two compactions ago arrives late and is shown as a run |
| Sequence numbers (`Unnumbered`) | An entry whose predecessor has not arrived is shown as a run, then merged when it arrives |
| Forks found through certificates (`Anchors`) | Once the original has moved two entries past a fork and compacted, only a reader's memory still shows the fork, and a reader without it merges the original's branch |
| The grace step's guard (`Unguarded`) | A reader that received a compaction's deletions before its snapshot stops merging a writer whose newer entries it already sees |
| `CheckFirst` (`Unchecked`) | A writer appends after an entry a restore took |
| Quarantine of dismissed entries (`Unquarantined`) | A reader that never saw a fork merges the branch a dismissal it holds names |

Properties checked, beyond chain order, single-writer directories, retention and
recovery: every seen entry is merged, held back, shown or dismissed; nothing is
both merged and shown; no entry is placed on top of a tip that folds it; a run
appears only for a fork, a loss, or what a reader still holds of one; at rest,
every entry no fork precedes is merged, readers with and without a cached view
merge and show the same, and remembered fork points are ones the files show; no
two branches are merged; dismissals hold; a reader stops merging only by
showing, losing, or the guarded grace step; a run stays shown until dismissed,
gone or the grace step; a writer retires on a fork only when an entry of its
directory has two successors; and every reader converges under fair delivery.

**Results.** The fast set (`nix build .#nord.toshokan-spec`) passes:
`Portable` (a clone, compaction, crashes; 186k states), `Recovery` (450k),
`Liveness` (329k), `Grace` (a restored folder and the grace step; 377k),
`GraceChaos` (the grace step with a sync fault; 293k), `LivenessGrace`
(convergence with the grace step and a restore; 56k), `RestoreDir` (one directory
restored; 423k), `Resurface` (a deleted segment arriving after later
compactions; 1.3M) and `Drop` (a sync client keeping an earlier version; 23k).
Every must-fail config fails with its stated property. Run once in the deep set:
`Anchors` fails as intended (7.1M states), `PortableWide` passes (3.0M),
`Restore` passes (5.6M), `RestoreFork` passes (8.8M), `RestoreDirFork` passes (a
clone and a restored directory; 8.8M), and `Dismiss` passes (a clone and
dismissals, four entries; 21.2M).
`Branches`, where both branches of a clone compact past each other, is too large
to explore whole; a random simulation of 93M states to depth 40 found no
violation.

**Values.** The model merges entries, not values. References, deletes that kill
fields, contradicted references, forgotten bases and the display of runs are
checked by `tests/retention`, against an oracle that holds every entry ever
written, never compacts and merges by exact hash. Histories of up to three
writers and a clone, which may resume mid-segment, write two entities'
existence, two registers and two set members, delete and revive, dismiss and
take runs, and compact after any entry, or after a sync client dropped the open
segment's newest line. Each writer writes from what the design under test shows
it of the folder. Every folder they leave is read by fresh readers, by cached
readers in every delivery order up to a cap (with deleted files delivered early,
late or never, conflicted copies, and a grace step halfway), at every subset of
its files, after a restore of the folder or of one directory to every earlier
state, and after every dropped version of a segment.

The properties: readers at rest show what the oracle shows; a replaced value
never shows alone again along a cached reader's deliveries, unless what replaced
it was dismissed or lies beyond a fork, or the field carries an item or a
contradiction report; a branch's step changes no merged value outside a
forgotten basis; no reader merges what a dismissal names; a contribution stays
within one own write and one reference per field and writer; and after a fault
and the grace step, the folder wins, beside what the guard keeps because
something visible descends from it. An extra value fails unless the oracle's
value shows beside it, after a fork or, after a fault, on a field the reader
reports contradicted, or the field is a forgotten basis. A missing value fails
unless an item of its writer shows it on that field, or shows that field as a
forgotten basis while no tip holds the write that carried it.

The design is a `Design` implementation that sees only what a reader sees, with
switches that drop the rules for branch references, the union of tips, own
writes and dismissals; with each switch off, a test must find a violation. The
same tests run against toshokan once it implements the design.
`cargo test --test retention_slice`, in the default suite, runs a bounded slice
in about two minutes, or half a minute in release.
`cargo test --release --test retention_sweep -- --ignored` runs the rest, 7.7
million histories, in about an hour.

Clocks: nothing in this design depends on them.

## Open questions and risks

1. **Certificate growth.** One per segment ever written, forever. Bounded by
   segments, not entries, but unbounded; a writer that starts many short
   processes pays the most.
2. **A slow sync and a quiet writer.** A writer that wrote nothing after
   compacting leaves nothing that descends from its snapshot; a sync slower
   than 24 hours makes its facts disappear until the snapshot arrives.
3. **Contradicted references** rely on the grace step to tell a lost write from a
   late one; before it, a late write's predecessor stays hidden. After it, a
   snapshot's reference that named the lost write no longer covers what earlier
   references of that writer covered, which comes back as a conflict.
4. **Forks become user work.** Every copied local root that wrote produces runs
   to review, even when both sides were active for seconds.
5. **A forgotten basis.** A branch that compacts past a pre-fork write it
   replaced takes that value out of the merge, and the field is shown in the
   branch's item until the user takes a value. Keeping each field's last few own
   writes in a contribution would make this rarer at a fixed cost per field; the
   exhaustive tests count how often it happens.
6. **A dropped version under a cached reader.** When a sync client drops the
   newest lines of a segment while a later line of the same chain survives, the
   guard keeps what the cached reader holds of the lost lines, and a fresh reader
   holds the survivor back as a gap; they disagree until the writer's next
   snapshot reaches both.
7. **A fork inside a segment.** When an instance is cloned while its segment is
   open and both sides compact, each tip names only the ends of that segment by
   certificate. The parting comes from every hash the tips name, so it reaches
   the last pre-fork entry that still has an effect; entries above it with none
   are shown as runs, and own writes that could be pre-fork are treated as a
   forgotten basis. Certifying segments, not lines, keeps that cost to display.
