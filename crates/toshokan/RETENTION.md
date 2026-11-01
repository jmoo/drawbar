# Bounded retention

This design bounds how much history toshokan keeps. [SPEC.md](SPEC.md)
specifies the format and protocol; this document gives the reasoning and the
guarantees. The model in [spec/](spec/) follows it, and `tests/retention`
checks toshokan against it.

It keeps only as much history as stops one instance from overwriting another's
work, whether the other runs beside it or on a computer the sync left stale;
history is not kept for version-control features. Undo and redo
reach only this session's intents and end at close. A reader keeps nothing
between opens: it derives what it shows from the files it sees. After a folder is
restored from an older copy, the folder wins at once, and nothing asks the user
to let the facts the restore took go or adopt them. Unsaved edits stay local to
each install. A write whose basis was forgotten is shown as a conflict, never
resolved silently either way, and there is no time after which an offline
writer's work stops counting. A delete kills the fields it observed, so reviving
an entity in a deletion conflict brings back only the concurrent edit.

## 1. Entries

Every entry carries `n`: 0 for the genesis entry, its predecessor's plus one after.
A reader does not place an entry whose `n` is not its predecessor's plus one, and
reports it.

Every reference — in `replaces`, `observed` and `tags` — is `[writer, n, hash]`.
An `add` carries `replaces`: the tags of its value it saw, its own included, so a
member has at most one live tag per concurrent adder.

A `dismiss` op names runs the user dismissed, and a `take` names the run whose
change an op takes (section 5). Each entry also records the fork points its
writer knew of, by writer and number.

## 2. Snapshots

A snapshot holds what its writer still contributes, plus what lets readers
recognize every entry it folds:

| Member | Value |
| --- | --- |
| `writer`, `label`, `at` | Its writer, the writer's label, and its clock reading |
| `genesis` | The hash of the genesis entry |
| `n`, `head` | The number and hash of the last folded entry |
| `certified` | Certificates, below |
| `state` | The contribution, below |

**Certificates.** A snapshot certifies every entry it folds, so that whenever a
file holding one reaches a reader, under any name, the reader skips it:

- by a segment certificate, where its writer knows a segment its chain was
  written to: the segment's first hash, and the hash and position of the last of
  its lines the snapshot folds;
- by hash, every other folded entry: one no segment it knows of holds, as after
  a sync client dropped a file's newest version;
- by hash, the last entry each of its dismissals names on its chain, so a reader
  can tell which branch a dismissal meant after the segment is gone.

The by-hash rule is what makes certification complete; segment certificates only
keep it small. Its writer knows segments from its own last snapshot, from the
files it read or wrote since, and from the files in its directory now. A segment
it does not know of costs hashes, not correctness. A file that ends before a
certificate of its previous snapshot with its first line reaches gets no
certificate of its own: it may be a dropped version of that segment, whose lines
past the first the writer cannot name, and a shorter certificate would make
them a fork in every longer copy.

Certificates are kept forever, and they are one of the two parts of a snapshot
that grow with use. A file sync brings back, from a backup or a device that was
away for a year, must be recognized as folded however old it is, and a fork that
reuses entry numbers must still be told from it, so a tip keeps a fingerprint of
every segment its writer wrote. A segment ends at every close of a process that
appended, at every compaction and at each MiB, and its certificate takes about
92 bytes.

The other is `settled`: a pair, about 70 bytes, for every pending record the
writer closed, one per commit that moved files and per settlement. A record
file sync brings back must still read as closed once the entry that closed it is
folded, and the entry after a record's position need not be the one that closed
it where a clone wrote first, so only the pair tells. Nothing else in a snapshot
grows with history, and nothing in it grows with the library's size.

A snapshot's head is among the entries it certifies, as the last line it folds
of a segment or by hash. A snapshot that carries the certificates of the
snapshots of its directory on its chain therefore certifies their heads, which
a resurfaced copy of one is recognized by.

**Matching a file to a segment certificate.** A file matches when its first line
has the certificate's first hash and its line at the certified position has the
certified hash; its lines up to that position are folded. An unsealed file that
starts with the first hash but is shorter has its lines past the first held back
as unplaced (section 4): the reader cannot tell a version a sync client
dropped, or one it is still bringing, from a copy that diverged before its end.
They are not evidence of a fork. So the two ways a copy diverges are not shown
alike: an unsealed copy that stops short hides its write to a field the other
copy wrote at a higher number, while one a line longer, or sealed, is a run and
shows everything it wrote. A sealed file cannot still be arriving, so a sealed
file shorter than the certificate diverged from it, as does a file whose line at
that position has another hash, as copies of a virtual machine resumed twice do:
only its first line is folded, and the rest is placed or shown as a run by the
rules below. A certificate names a segment by its first line and a position, so
a shorter unsealed copy of a certified segment cannot be told from another
instance's copy that left the chain there; where a certificate of that other
copy matches it, the reader takes its lines as a branch and shows them as a run,
never merging them silently.

**Contribution**, per entity:

- **Registers, the file register, existence.** The writer's latest own write to
  each, live or not: whether it survives is decided at merge time, by references,
  as for every other write. With it, the number of the writer's first own write
  to it, which tells a forgotten basis from none (section 4).
- **Sets.** Per member, the writer's latest own add or remove, live or not, and
  the number of its first.
- **References.** Per register and per set member, and for each other writer,
  the latest `[own n, own hash, writer, n, hash]` at or below the fork point of
  that writer its writer knows of, and the latest above it: the target the
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

References are kept per other writer and side of a fork point. A reference made
before its writer saw a fork, to a write above it, moves above it when the
writer next makes a reference to that writer on that field, so a later one below
the fork does not overwrite it. Above a fork point, a reference to one branch
can be overwritten by a later one to the other. Such a reference no longer
applies, and the write it killed is shown in its run, or, to a reader that does
not see the fork, beside the writer's value as a conflict: shown, never lost. A
writer that has seen the fork keeps one reference on each side of it, so a take,
which names both the merged write and the run's, keeps both.

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

**A contradicted reference.** If a reference names a number above the highest
the files show for its target writer (a restore of one directory, a dropped
version, or an entry sync has not brought yet), or a number at which the files
show only another entry of that writer (a restore that took one branch), every
reference of the same entry or contribution to that writer kills only the write
with its exact hash. What only the missing write replaced then shows again, as
it would for a reader holding every entry. Where a snapshot holds the reference
instead of its entry, the earlier exact targets are gone, and a write the
missing one did not replace can come back too: beside the current value as a
conflict, never in place of it. While sync is still bringing the missing entry,
the conflict lasts until it arrives.

## 4. Placing entries

Per writer `y`, from the files the reader sees:

- **Tips.** The snapshots of `y` no other snapshot of `y` certifies.
- **Folded.** An entry a tip certifies by hash or head, or a line of a file that
  matches a segment certificate (section 2). Skipped. This recognizes
  re-delivered files, late deletions, conflicted copies, crash leftovers,
  resurfaced files and the folded part of a segment that runs on past a tip's
  head.
- **Unplaced.** The lines past the first of an unsealed file that starts like a
  certified segment but is shorter (section 2). Shown as an item, without a
  line's write to a field its writer's merged history writes at the line's
  number or above, which already decided it. The user can take its changes, or
  dismiss it by naming exactly its lines.
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
numbered above it, is a run: no branch is merged beside another. Certificates
make every fork point derivable from the files, so a reader finds it again
whichever branch's files it sees first, except for a fork inside a segment (see
What users see).

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
writer that had not seen the fork, does not. It arises whenever a branch compacts past a pre-fork write it
replaced, so a branch's compaction can change the merged value of such a field.
An entity left with no live existence write this way shows its live fields, as
in a deletion conflict.

**A function of the files.** A reader's placement depends on the files it sees
and nothing else, so readers that see the same files agree, whatever order they
arrived in.

Re-deliveries are recognized by certificates, and a fork near the head is found
by the two successors of its fork point.

## 5. Runs

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
a new item, never placed, whether or not the reader sees the fork: so are a
tip's writes after a dismissed entry on its chain, and the entries after that
tip's head. A tip's run starts at the first of its entries the merge leaves out.
A dismissal never covers what the user has not seen. A
tip matches a dismissal by the hash of its last entry where the tip names it. A
tip that compacted past a line-run's last entry inside one segment names it
nowhere, and two branches of one instance resumed twice can share that segment's
first line, so nothing tells such a tip's branch: it is shown again as an item,
to dismiss again, rather than hidden on a guess.

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

## 7. Restores and sync in flight

A reader keeps nothing between opens, and nothing between reads that the files
it sees do not hold. Every open reads every file of every writer's directory.

**A restore.** After the folder, or one writer's directory, is restored from an
older copy, a reader shows what the restored files say at its next read. Facts
only the restore took disappear, with no prompt; a reference to an entry the
restore took is contradicted (section 3), so what only that entry replaced comes
back beside the current value. A writer whose last entry the folder lost retires
(`CheckFirst`).

**Sync in flight.** A sync client delivers files in any order, so a reader can
see less than the folder holds:

- A compaction's deletions can arrive before its snapshot. Until it does, the
  writer's facts the deleted segments held are not shown, and the writer's later
  entries, which follow the missing snapshot's head, wait as a gap.
- A segment can arrive before the entries it continues: a gap.
- A segment still arriving, shorter than a certificate, holds its lines past
  the first as unplaced (section 2) until it is whole.
- A reference to an entry sync has not brought is contradicted (section 3):
  what it replaced shows beside the current value as a conflict until the entry
  arrives.

What disappears comes back when its file arrives. A change the user makes
meanwhile is judged against what is shown, so at worst it produces a visible
conflict: references are never dropped, so nothing it replaced comes back
alone, and nothing another writer wrote is lost.

A snapshot lost by hand, or by a sync fault, while a newer segment survives
leaves that segment held back as a gap until the writer's next snapshot reaches
it. A dropped version of a segment holds its surviving lines past the first as
unplaced, which show only what its writer's later history never decided.
Neither is something the protocol does.

## 8. Writers, pending records and undo

A writer continues only if its recorded head is its tip or placed after it, and
the snapshot's `genesis` is its genesis; otherwise it retires. A writer that
retires while its instance runs, other than over a fork, empties its trash
first, so retiring again and again strands no trash. After one such retirement
over a write that landed though it was reported failed, a failed write of the
next writer turns the library read-only rather than retire it too, so a folder
that makes no write durable retires at most one writer per open.

An open pending record's `prev` is its writer's last entry, so it is always held.
A record whose `prev` is followed, or numbered below a tip's head, is spent or
closed, unless `prev` is a run, on a branch the tip's number says nothing of.
One whose `prev` is neither held nor in a run is ignored and reported.

The undo stack lives in memory and holds this session's intents only. Each item
keeps what reversing it needs: the values its writes replaced, the tags its
removes named, its file effects. It ends at close.

## What users see

- The library as every writer's live writes say, with conflicts where writes are
  concurrent.
- After a copied local root, VM resume or restored local-root backup: one item per
  branch of that writer's history after the fork, with the net change it makes,
  to keep or dismiss. Neither branch's later changes are merged until the user
  chooses; the writers involved retire and continue under new ids.
- After a folder restore: the folder as restored, at the next read. The facts
  the restore took disappear, without a prompt.
- While a sync client is delivering: facts can disappear and come back, as
  when a compaction's deletions arrive before its snapshot, and a field can show
  a conflict until the entry a reference names arrives. A change made in that
  window is judged against what is shown: at worst a visible conflict, never a
  silent loss, and never a replaced value shown alone again.
- A field another writer emptied, or an entity another writer deleted, while
  this one edited it: a register conflict, or the entity shown in a deletion
  conflict with only the concurrent edit.
- A change from another branch of a forked writer whose file sync hides: gone
  from view, with no item, until its file is visible again, then shown as that
  branch's item.
- A run the user dismissed, after its branch compacted past it: its branch shown
  again as an item.
- Entries after a gap: an item from the start, whose changes the user can take,
  merged as soon as the gap fills.
- A field whose earlier value a branch compacted away: shown in that branch's
  item until the user takes a value, even when it matches the merged one.
- A reference whose target a restore took kills only that target, and what
  earlier references of its writer covered, that a snapshot holding the
  reference no longer names, comes back beside the current value as a conflict.
- A segment a sync client dropped the newest lines of: its surviving lines past
  the first held back as unplaced, which show only what the writer's later
  history never decided again, usually nothing, and a later line of the same
  chain held back as a gap, until the writer's next snapshot reaches both.
- A fork inside a segment, when an instance copied while its segment was open
  and both copies compacted: pre-fork entries with no effect left can show as
  runs, and own writes that may be pre-fork as a forgotten basis.
- Every copied local root that wrote produces runs to review, however briefly
  both copies ran.
- Undo reaches this session only.

## Guarantees

**Kept.**

- Readers that see the same files merge and show the same entries, at once,
  whatever order the files came in; after a restore, what the restored folder
  says.
- Every kept entry is merged, shown as a run, dismissed by the user, or held back
  after an entry a restore or a dropped version took.
- An entry no fork precedes and that follows nothing lost is merged.
- No two branches of a writer are ever merged together.
- A run stays shown until it is dismissed or its files go.
- A step by one branch of a forked writer changes no merged value, except on a
  field shown as a forgotten basis.
- A replaced value or removed member never comes back without a conflict, also
  while sync is in flight: references are never dropped, a write a reference
  covers stays dead, and a reference the files contradict shows what it
  replaced only beside the current value.
- Chain order, single-writer directories, no bytes leaving the folder except by
  emptying a trash, total recovery, opening writing nothing.

**Not kept.**

- A fork's branches are not merged side by side: each branch after the fork
  point is a run for the user.
- What a reader shows does not only grow while sync delivers files out of
  order: facts can disappear and come back.
- A branch that compacts past a pre-fork value it replaced takes that value out
  of the merge, and the field shows as a forgotten basis.
- Reviving a deleted entity brings back only the concurrent edit.
- A reader may show a field empty, or as a conflict, until the write that
  replaced the value it showed arrives.
- A copy of a segment that diverged and stops short, unsealed, hides its write
  to a field the other copy wrote at a higher number; a copy one line longer is
  a run, and shows it.

## Expected size

Steady state, for the schema the harness writes:

| Part | Memory | Folder (JSON) |
| --- | --- | --- |
| Live writes | ~100 B each, besides the maps that hold them | ~110 B each, ~1.1 KB per entity |
| References | ~48 B each | ~90 B each |
| A deleted entity | ~100–150 B, forever | the same |
| A removed member never re-added | one reference per remover, forever | the same |
| Certificates | ~60 B per segment ever written | ~90 B per segment ever written |
| Records closed | ~70 B per commit that moved files and per settlement | ~70 B each |
| Entries since the last snapshot | up to the compaction threshold, ~400 B each | ~250 B each |

References exist only where one writer replaced another's write: at most
(writers − 1) per field, and in practice far fewer.

For 2,000 entities, 3 writers and a compaction threshold of 1,024 entries, the
steady state is about 6 MB of memory and 4 MB of folder, plus:

- per deleted entity, ~150 B;
- per segment ever written, ~90 B: a segment holds up to 1 MiB, about 4,000
  entries, and every process that writes starts one, so 2M entries over years of
  daily use come to some thousands of segments, a few hundred KB;
- per pending record the writer closed, ~70 B: one per commit that moved files
  and per settlement;
- per retired writer, its final contribution and the references others keep to
  it, at most what it wrote, and the trash items its open records name; one that
  retired over a fork, or as an instance opened, also keeps the rest of its
  trash.

These grow with deletions, segments, commits that moved files and writers, not
with entries otherwise.

`tests/no_growth.rs`, in the default suite, checks this for ten entities at
10,000 and 100,000 entries: the folder, what open reads and keeps, and the
requests of open, commit and refresh stay the same, but for the certificates and
the record pairs the entries between add, and a digit per number.

## An old folder

There are no migrations before 1.0. This format lives under `writers.2/`; an old
folder opens as an empty library whose files are all unbound, and the old
directories are left for older builds. A local root's `view.bin` and `view.log`,
which older builds kept, are ignored, as are old `head.json` files.

## The model

[spec/Portable.tla](spec/Portable.tla) checks placement, forks, runs,
certificates, dismissals, restores of the folder and of one directory, and
dropped versions, over an abstract merge (a reader's state is the set of entries
it merged, derived from the files it sees). See [spec/README.md](spec/README.md)
for the rules, properties and models.

Its rules are constants, so a model can drop one and show what it prevents:

| Without | What goes wrong |
| --- | --- |
| Certificates (`Uncertified`) | A writer cannot delete a segment below its snapshot's anchor, and readers show that segment's folded lines as a run |
| Carrying certificates forward (`Uncarried`) | A segment deleted two compactions ago arrives late and is shown as a run |
| Sequence numbers (`Unnumbered`) | An entry whose predecessor has not arrived is shown as a run, then merged when it arrives |
| Forks found through certificates (`Anchors`) | Once the original has moved two entries past a fork and compacted, a reader that still sees the original's deleted segment finds the fork, and one that does not merges the original's branch |
| `CheckFirst` (`Unchecked`) | A writer appends after an entry a restore took |
| Quarantine of dismissed entries (`Unquarantined`) | A reader that never saw a fork merges the branch a dismissal it holds names |

Properties checked, beyond chain order, single-writer directories, retention and
recovery: every seen entry is merged, held back, shown or dismissed; nothing is
both merged and shown; no entry is placed on top of a tip that folds it; a run
appears only for a fork or a loss; at rest, every entry no fork precedes is
merged, and a reader merges, shows and finds the fork points that a reader of
the folder's files alone does, whatever deleted files it still sees; no two
branches are merged; dismissals hold; as files arrive, a reader stops merging an
entry only by showing or dismissing it, and a run stays shown until dismissed; a
writer retires on a fork only when an entry of its directory has two
successors; and every reader converges under fair delivery.

[spec/models.nix](spec/models.nix) gives each model its suite, fast or deep.

**Values.** The model merges entries, not values. References, deletes that kill
fields, contradicted references, forgotten bases and the display of runs are
checked by `tests/retention`, which runs toshokan's encoding, compaction and
derivation against an oracle that holds every entry ever written, never
compacts and merges by exact hash. Histories of up to three
writers and a clone, which may resume mid-segment, write two entities'
existence, two registers and two set members, delete and revive, dismiss and
take runs, and compact after any entry, or after a sync client dropped the open
segment's newest line. Each writer writes from what toshokan shows it of the
folder. Every folder they leave is read at rest, along every delivery
order up to a cap (with deleted files delivered early, late or never), with
conflicted copies, at every subset of its files, after a restore of the folder
or of one directory to every earlier state, and after every dropped version of a
segment. A reader holds nothing between reads.

The properties: readers at rest show what the oracle shows; a replaced value
never shows alone again along a reader's deliveries, unless what replaced it was
dismissed or lies beyond a fork, or the field carries an item or a contradiction
report; a branch's step changes no merged value outside a forgotten basis; no
reader merges what a dismissal names; a contribution stays within one own write
and one reference per field and writer; and after a fault, the folder wins, but
for a value that a line the folder holds replaced on its own chain, through the
entry the fault took. An extra value fails unless the oracle's
value shows beside it, after a fork or, after a fault, on a field the reader
reports contradicted, or the field is a forgotten basis. A missing value fails
unless an item of its writer shows it on that field, or shows that field as a
forgotten basis while no tip holds the write that carried it.

`tests/retention_slice.rs`, in the default suite, runs a bounded slice;
`tests/retention_sweep.rs` runs the rest by name.

Nothing in this design depends on clocks.
