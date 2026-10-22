# Bounded retention (draft)

This draft revises how much history toshokan keeps. It will be folded into
[SPEC.md](SPEC.md) once settled; the model in [spec/](spec/) already follows it.
No Rust code implements it yet.

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

A `dismiss` op names runs the user dismissed (section 5). `reverses` is dropped.

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

**Certificates.** A snapshot certifies:

- for every segment its chain was ever written to, whether the folder still holds
  it or not: the segment's first hash, and the hash and position of the last of
  its lines the snapshot folds;
- the head of every snapshot of its directory on its chain;
- by hash, each folded entry no segment held when it compacted, as after a sync
  client dropped a file's newest version.

Its writer takes them from four places: its own last snapshot, which its cached
view holds even after another instance of the same writer deleted the file; the
snapshots of its directory on its chain; the files it read or wrote since its
last snapshot, which its cached view records; and the files in its directory
now. Certificates are kept forever. Together they make every entry a snapshot folds
recognizable whenever a file holding it reaches a reader, whatever its name.

**Contribution**, per entity:

- **Registers, the file register, existence.** The writer's own write that
  survives in its view: at most one per register, because each own write
  replaces every survivor it sees, its own earlier write included.
- **Sets.** Per member, the writer's own add that survives in its view: at most
  one.
- **References.** Per register, and per set member, the latest `[n, hash]` of
  each other writer's write or add that its entries replaced or removed, carried
  forward across its own later writes. A delete keeps, per other writer, the
  latest `[n, hash]` among the existence writes and field writes of the entity it
  observed.

Plus trash items not yet emptied, settlements, dismissals, and its own unknown
and extended entries.

A write dead in the writer's view is dropped from its contribution only when a
file it sees holds the reference that killed it, so knowledge held only in a
cached view after a restore is never published. A dead delete or remove is kept
for its references; nothing else of it is needed. References are never dropped.

## 3. The merge

A reader's merged state is the join of every writer's contribution: its tip
snapshot and the entries placed after it. A write or add is **live** while its
writer's contribution holds it and no reference kills it.

A reference `[y, n, h]` on a field kills every write or add of `y` on that field
numbered below `n`, and the one numbered `n` with hash `h`. One numbered `n` with
another hash is on another branch, and is not killed. The rule is sound because
`y`'s write `n` replaced every earlier write of `y` to that field it saw, and `y`
always sees its own; forks are never merged (section 4), so every merged write of
`y` lies on one chain.

| Type | A contribution keeps | An arriving write or add is |
| --- | --- | --- |
| Register, file register | Own live write; per other writer, the latest reference | Dead if a held reference covers it; it kills what its references cover |
| Set member | Own live add; per other writer, the latest removed tag | As a register, per member: an add's `replaces` and a remove's `tags` are references |
| Existence | Own live create or delete; a delete's references, kept even once dead | As a register. A live delete beside a live field write is a deletion conflict |

**Deletes.** A delete kills the entity's existence writes and field writes it
observed. A field write it did not observe stays live, so a concurrent edit
leaves the entity shown in a deletion conflict; reviving it brings back only that
edit. A delete keeps its references forever, so an edit made long after, by an
instance that had not read the delete, is still a deletion conflict.

**A newer snapshot of a writer.** It replaces that writer's contribution. A
cached reader keeps an own write of that writer the new snapshot no longer holds
until a reference arrives that kills it, or the grace step: the write that
replaced it always carries that reference. A reader without a cached view may
show the field empty until then; the two disagree on that field only until the
replacing write arrives.

**A contradicted reference.** If, after the grace step, a reference names a
number above the highest the files show for its target writer (a restore of one
directory, or a dropped version), it kills only the write with its exact hash,
and the reader reports it. The predecessor of the lost write then shows again, as
it does today.

## 4. Placing entries

Per writer `y`, from the files the reader sees and what it remembers:

- **Tips.** The snapshots of `y` no other snapshot of `y` certifies.
- **Folded.** An entry a tip certifies, or a file whose first hash matches a
  certificate, up to its certified line. Skipped. This recognizes re-delivered
  files, late deletions, conflicted copies, crash leftovers, resurfaced files and
  the folded part of a segment that runs on past a tip's head.
- **Placed.** An entry whose predecessor is a tip's head or placed, or a genesis
  entry when `y` has no tip.
- **Gap.** Any other entry whose predecessor may still come after the tips'
  heads: `n − 1` above the highest tip head. Held back and reported.
- **Run.** Any other entry, and every entry after one.

**Forks.** A fork point is an entry with two successors among those placed or
seen, or an entry a tip certifies that is the predecessor of an entry no tip
folds (the tip's chain holds the other successor). Every entry of `y` numbered
above a fork point, on either branch, and every live write in a tip of `y`
numbered above it, is a run: no branch is merged beside another. A reader
remembers the fork points it found, so a fork stays a fork when one branch's
files go before the other's; certificates make every fork point derivable from
the files again, so a reader without that memory finds the same ones.

**Several tips.** When two snapshots of `y` do not certify each other, the
writer forked and both sides compacted. The highest line every tip certifies is
their fork point. The reader merges only the live writes every tip holds with
the same hash, and shows each tip's other writes as a run, listing only those
numbered above the fork point: below it, a tip's writes are the shared past the
other branch has since changed.

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
virtual machine resumed twice, a restored backup of a local root. A gap that is
still open after the grace period and a full rescan becomes a run too: the entry
it waited for is gone, as after a sync client dropped a version.

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
against the current state, and a `dismiss` op names the run as
`[writer, first hash, last hash]`, or a tip run by its head. No reader shows what
a dismissal names again. Entries the branch adds after the last hash show as a
new item: a dismissal never covers what the user has not seen.

**Retirement.** A writer takes a new id only when it sees an entry of its own
directory with two successors. A run in its directory without such a fork, such
as an expired gap, does not retire it.

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
drops each such item of which it sees nothing newer of that writer: no entry or
snapshot with a greater number. It also forgets that writer's fork points the
files no longer show, as when a restore took one branch. It then places that
writer's files again. Facts only a restore took disappear, with no prompt.

The guard is what tells a restore from a slow sync. After a compaction, a reader
may receive the deletions before the new snapshot; the writer's later entries,
numbered above the missing snapshot, show it has moved on, and the reader keeps
waiting. A writer that wrote nothing since compacting gives no such sign: after
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
- A gap that never fills: held back and reported, and after the grace period
  shown as a run.
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
- A replaced value or removed member never comes back without a conflict:
  references are never dropped, and a write a reference covers stays dead.
- Chain order, single-writer directories, no bytes leaving the folder except by
  emptying a trash, total recovery, opening writing nothing.

**Given up**, and what allows it.

| Was | Now | Allowed by |
| --- | --- | --- |
| A fork's branches are merged side by side and reported | Each branch after the fork point is a run for the user | D14 |
| The cached view only grows | The grace step drops what no file holds and nothing newer contradicts | D12 |
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

**Results.** The fast set (`nix build .#nord.toshokan-spec`, about 7 minutes) passes:
`Portable` (a clone, compaction, crashes; 186k states), `Recovery` (450k),
`Liveness` (329k), `Grace` (a restored folder and the grace step; 377k),
`RestoreDir` (one directory restored; 423k), `Resurface` (a deleted segment
arriving after later compactions; 1.3M) and `Drop` (a sync client keeping an
earlier version; 23k). Every must-fail config fails with its stated property.
Run once in the deep set: `Anchors` fails as intended (6.4M states),
`PortableWide` passes (3.0M), `RestoreDirFork` passes (a clone and a restored
directory; 8.8M), and `Dismiss` passes (a clone and dismissals, four entries; 21.2M).
`Branches`, where both branches of a clone compact past each other, is too large
to explore whole; a random simulation of 93M states to depth 40 found no
violation.

**Not modeled.** Values: references, deletes killing fields, contradicted
references and the trimmed display of tip runs need the exhaustive tests of
`tests/facts.rs` — every arrival order, split and compaction of bounded histories
for three writers, one clone, registers and sets, and one partial restore —
checking that a replaced value never shows without a conflict on its field, that
every placed write is merged or shown, that readers with the same files show the
same values, and that while one branch writes the other's merged values do not
change. Gap expiry after the grace period. Clocks: nothing in this design depends
on them.

## Open questions and risks

1. **Certificate growth.** One per segment ever written, forever. Bounded by
   segments, not entries, but unbounded; a writer that starts many short
   processes pays the most.
2. **A slow sync and a quiet writer.** A writer that wrote nothing after
   compacting gives the guard nothing newer to see; a sync slower than 24 hours
   makes its facts disappear until the snapshot arrives.
3. **Contradicted references** rely on the grace step to tell a lost write from a
   late one; before it, a late write's predecessor stays hidden.
4. **Forks become user work.** Every copied local root that wrote produces runs
   to review, even when both sides were active for seconds.
5. **The merge rules are not model-checked.** References and deletes that kill
   fields are where an error would resurrect or lose data; the exhaustive tests
   above must exist before code depends on them.
