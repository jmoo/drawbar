# toshokan protocol

This document specifies how toshokan's writers and readers behave.
[format.md](format.md) specifies the bytes they write and read.
[README.md](README.md) describes the TLA+ model of the protocol between writers
and readers, and maps its properties to the tests that check the code.

## Durability

A backend says, for each of the folder and the local root, whether it can sync.
Where it can, a file's contents are durable once a sync of the file returns,
and a name made or removed once a sync of its directory returns, except in a
browser, which keeps names where toshokan cannot sync them (see Browsers).
Where it cannot, toshokan counts a request as durable once it completes: what
completed survives the app stopping, and what survives the machine or the
browser stopping is the backend's business. A write to a file of a picked folder
is the exception: it completes before it lands, and lands when the file's stream
closes, before the backend performs any later request other than another write
to that file or a read of another file (see Browsers). Stopping the app before
then loses the write, as if it had not completed. Every use of durable in this
document means this, and nothing toshokan reports promises more.

## Merging

A reader merges what each writer still contributes: its tip snapshots' state
and the entries merged after them (see Reading). The merge is a function of
those alone, whatever order the files arrived in. Every write is named by its
writer, number and hash. An entry's ops apply in order, so where two of them
write one field, the later is the entry's write.

Keys are declared by the app as registers or sets, each under a name. A key's
values are the app's JSON. A field is an entity's existence, one of its
registers, one member of one of its sets, or its file register. A write is
**live** unless one of these kills it:

- **Own writes.** A writer's write on a field kills that writer's writes
  numbered below it on that field.
- **Own deletes.** A writer's `delete` kills that writer's field writes of the
  entity numbered at or below it, its own entry's included.
- **References.** A reference on a field to `[y, n, hash]`, in `replaces`,
  `tags` or `observed`, kills on that field the write of `y` with that hash and
  every write of `y` numbered below `n`. A write numbered `n` with another hash
  is on another branch, and is not killed. Where `n` lies above a fork point of
  `y`, the reference kills only the write with its hash. A reference made by an
  entry numbered above its own writer's fork point, or dismissed, applies only
  within its run.
- **Takes.** An op whose `take` names a run of `y` kills, on its field, every
  write of `y` numbered below the run's `from`.

What the live writes show:

- **Registers** show the values of their live writes. Live writes of different
  values are a conflict; equal values are one value. A reader shows the latest
  by clock reading, then by writer id, and lists the rest. A write that replaces
  every live write resolves the conflict. A live clear holds no value, so a
  clear concurrent with a value leaves the value.
- **Sets** hold each member with a live add. An add the remover had not seen
  stays live.
- **Existence** is a register whose writes are `create` and `delete`. An entity
  is shown while a create is live, and in a deletion conflict while a delete is
  live beside a live create or a live field write that holds something: a
  register value, a member or a file. A `create` that replaces the delete
  settles it in favor of the entity. An entity with no live existence write,
  which only a forgotten basis or a lost write leaves, shows its live fields.
- **Files** are a register per entity whose writes are `file` and `pin` ops.
- **Trash items** are kept per writer and item from the `displaced` lists, and
  **settled records** from the `settle` entries.
- **Unknown** entries and ops are kept with the entry that holds them. An entry
  of a known kind is merged and also kept whole when its JSON holds a member,
  at any depth, that the reader leaves out of what it decoded. Member order and
  spacing do not count.

A value that does not decode as the key's declared type is shown as unreadable,
and kept and merged like any other.

## Writers

The writers of an install are the directories of its local root, its pool. An
instance takes the first, in order of name, whose lock it gets and whose
`head.json` reads and is not retired. It continues that writer only if what it
has read of the writer starts from its genesis entry, holds its recorded head,
has one last entry and no fork point, and a file in its directory holds that
last entry now. Otherwise it marks `head.json` retired, releases the lock, and
writes as a new writer from its next write: a copied local root, whose history
another instance also continues, and a folder restored from an older copy, which
no longer holds the writer's last entry, each start a new writer. An instance
that finds its own history forked while it runs does the same.

An instance also takes, as it opens and at each refresh, the lock of every other
writer no instance writes as now, as another window of the app wrote until it
closed, and keeps it while that writer left pending records open: it reports
them as orphans that are the install's own (`ours`), to settle without asking.
It lets go of such a lock at once when the writer left none, a record its own
log closed counting as none, and as soon as its records are settled, so the next
instance writes as it and the pool stays as large as the instances that run at
once. An orphan whose writer's lock another instance on this machine holds is
`live`: that instance may be carrying it out still. Asking which writers other
instances hold tries only the locks this instance does not hold itself.

A writer forks when two instances write as it: a copied local root, a virtual
machine resumed twice, a restored backup of a local root. Each fork point is
reported once. The writer retires whenever its own instance's reader holds a
fork of its history, which opening, a refresh, or a compaction refused with
`Rekey::Forked` reports; an instance never compacts a history it knows forked.
A gap in its directory does not retire it.

A new writer is created at an instance's first write. Its first segment holds
its genesis entry; the segment, its directory and every directory above it are
synced. Only then does the writer get its directory in the local root, its lock
and its `head.json`.

A process appends to one segment, named at random when it first appends, and
seals it when it closes it, compacts, or would take it past 1 MiB with an
append, which then goes to a new segment: once the segment still has the length
this process left it and ends with its last entry's line, it appends a seal
marker and syncs the segment. Nothing is appended to a sealed segment. Before
each append, a writer confirms that the folder holds its last entry: its open
segment has the length this process left it and ends with that entry's line.
Otherwise, or with no segment open, a file in its directory must hold that
entry, and the writer leaves the segment without a seal marker and appends to a
new one. It looks first at the file it last knew to hold the entry, unchanged in
length and last 34 bytes, then at every file whose last 34 bytes end that
entry's line, and only then reads files whole. If no file holds it, the writer
writes nothing and stops: a new writer takes over from the next write. A writer
whose write failed, or whose sync of it failed, checks the file it wrote before
anything else of its next write is carried out; if the first entry of the write
landed whole, it stops in the same way, and the entries that landed stand. Once
one writer of an instance stopped so, or failed to be created, a failed write of
a writer that took over, its creation included, turns the library read-only
until it opens again (`Why::NotDurable`). A write refused for want of space is
no such failure: it fails as it is. After the append is synced it stats the
segment and writes `head.json`, before the commit returns. An append that fails
leaves the segment without a seal marker and appends nothing more to it, so
nothing follows a torn line or an entry that may not be durable.

The entries are then durable, and the commit succeeds whatever fails after them
(see Intents). A `head.json` that cannot be written lags: the instance writes it
again at its next commit, refresh or close. A crash meanwhile loses nothing the
folder holds. A `head.json` that names an earlier entry of the writer's chain
still lets the instance continue the writer, after the chain's last entry; one
that a failed write tore starts a new writer.

## Reading

A reader lists `writers.2/` and reads every file directly in each `writers.2/<w>/`
other than a swap file, up to 256 MiB of it. A sealed segment or a snapshot whose
length and modification time are unchanged is not read again, since nothing
writes either again: segments and snapshots get new random names. A refresh
takes a sealed segment or snapshot read before by its name alone, and asks the
length and modification time only of the other files, so a file changed in
place, as a restore of the folder from an older copy can leave one, is found by
the next open or rescan, which ask them of every file. Any other file
whose length, modification time and last 34 bytes are unchanged is not read
again; a segment's last 34 bytes are the tab, hash and LF that end its last line,
or those of its seal marker. A segment without a seal
marker that grew is read from the end of its last line on, once the 34 bytes
before that point still end that line; any other changed file is read whole. A
file is a segment when it is empty or its first line can be read, else a
snapshot when it decodes as one; anything else is reported when the library
opens, and read again next time. A snapshot whose `writer` is not `w` is
reported, not used. A file another writer's directory lists that cannot be read
for any reason but its absence, as a file a sync client keeps only online, is
taken as the last read found it, or as not arrived yet, and reported and read
again next time; until then no orphan is reported or settled, since the entry
closing its record may be in that file. Where the file is in the directory of
the writer this instance writes as, opening or refreshing fails instead.

Per writer, from the files it reads, a reader derives:

- **Tips**: the writer's snapshots no other of its snapshots certifies, one per
  head.
- **Folded** entries, which it skips: one a tip names, or a line of a file whose
  first line and line at a segment certificate's position are the certificate's.
  This recognizes re-delivered files, late deletions, conflicted copies, crash
  leftovers and the folded part of a segment that runs on past a tip's head.
- **Unplaced** lines: those past the first of a file without a seal marker that
  starts like a certified segment but is shorter, held back: it may be a
  version a sync client dropped, a copy that diverged before its end, or a file
  still arriving. A sealed file shorter than the certificate, or one with
  another line at its position, diverged: only its first line is folded. A
  certificate names a segment by its first line and a position, so where a
  shorter certificate with the same first line matches the file, its lines past
  that one are a branch, shown as a run and never merged.
- **Merged** entries: one numbered one past its predecessor, which is a tip's
  head or merged, or a genesis entry when the writer has no tip. An entry
  numbered other than one past its predecessor is never merged.
- **Fork points**: an entry with two successors; a folded entry, not a tip's
  head, that precedes an entry no tip folds; the predecessor of a line no tip
  folds numbered at or below a tip's head, even where a segment certificate
  holds it and it cannot be checked by hash, since certificates are complete;
  and the parting of several tips, below.
- **Dismissed** entries: those a dismissal names, which no reader merges or
  shows, even once it stops seeing the fork. An own delete a tip holds that a
  dismissed entry made does not apply. An entry after one is a run, and so are a
  tip's writes after one on its chain and the entries after that tip's head. A
  tip's run starts at the first of its entries the merge leaves out: above the
  lowest fork point, or from the first a dismissal names on its chain.
- **Runs**: every entry and every tip's write numbered above the lowest fork
  point, on either branch, so that neither branch is merged; every other entry
  numbered at most one past the highest tip's head; and every entry after a run.
- **Gaps**: every other entry, whose predecessor may still come.

Where several tips of a writer do not certify each other, the writer forked and
both sides compacted. Their **parting** is the highest entry every tip names by
hash: in its certificates, as its head, as an own write, as the own entry of a
reference, as an own delete, or as a dismissal. Below the parting the reader
merges the union of the tips' own writes; above it, every tip's own writes and
references are runs. Two tips with one head are copies of one chain compacted
twice: their own writes and references are the same, and the reader takes the
union of the rest, trash items and settlements.

### Runs

What the merge leaves out is shown as **items**: each run, gap and file's
unplaced lines, with the net change it would make
against the merged state, field by field, how many entries it holds and the
earliest and latest of their clock readings. Of a run a tip holds, a reader
counts every number from the run's first entry no dismissal names to the tip's
head, and knows only the readings of the head and of the writes the tip keeps.
Readers that see the same files name each run alike (see
[Entries](format.md#entries) for the form).

An intent takes changes from an item with ops that name its run in `take`, and
dismisses a run or unplaced lines with a `dismiss` op; a gap cannot be
dismissed. A run is shown until it is dismissed, with no change once it would
change nothing; a gap only while it would change something, and unplaced lines
while they would and no dismissal names them exactly. Nothing about a gap
expires: when it fills, its entries are merged, and a change the user took from
it writes the same value, so the field shows one value. An unplaced line's write
to a field its writer's merged history writes at the line's number or above
changes nothing: that history already decided the field.

A dismissal by a tip's head also covers the writes numbered up to that head in
any later tip that certifies it. A tip matches a dismissal by the hash of its
last entry where the tip names it. A tip that compacted past a line-run's last
entry inside one segment names it nowhere, and is shown again as an item, to
dismiss again.

A tip's own write the merge leaves out, above the fork point or named by a
dismissal, may have replaced an own write of its writer the tip no longer holds.
It did when the tip's first-write number for its field is below the write's and,
for a write above the fork point, no higher than the fork point could be: where
several tips part inside a segment, just below the lowest head. When it did and
no merged write of that writer on that field is numbered below it, the write is
a **forgotten basis**: its run shows that field whatever the merged state shows,
and a dismissal does not hide it. Only an intent that takes a run of that writer
on that field settles it; a write that merely names the run's write, by a writer
that had not seen the fork, does not.

### What a reader holds

A reader derives what it shows from the files it sees and nothing else: every
open reads every file of every writer's directory, so readers that see the same
files show the same. While an instance runs, its reader keeps what it read of
each file, to read again only the files that changed. What only a file the
folder no longer holds gave is gone at the next read: while a sync client
delivers a compaction's deletions before its snapshot, until the snapshot
arrives, and at once after a restore of the folder from an older copy, with no
prompt.

A reference is **contradicted** when it names a number of a writer above the
highest the files show of it, or a number at which the files show only another
entry of that writer. It kills only the write with its hash, and so does every
other reference of the same entry to that writer; a reference a snapshot holds
is judged alone. What only the missing write replaced is shown beside the
current value until the rest arrives: a conflict, never a value shown alone in
place of a newer one. Where a snapshot holds the reference in place of its
entry, the earlier exact targets are gone, and a write the missing one did not
replace can come back too, beside the current value.

## Binding

Which library file is an entity's is derived, never logged as such. A binding is
a function of the merged file registers, the library files a reader sees and
their identities. After any change to the file facts or to the files a reader
sees, every fact is bound again:

1. A scan lists every library file with its length and modification time. It
   reads an identity only where it decides a binding: for a file at the path of
   a file fact of its length, and for a file of the length of a fact of an
   entity that may have moved. Such an entity is one none of whose file facts'
   paths holds a file that no other entity's fact names too, paths compared
   under the volume's rules for case and Unicode normalization. It reads none
   when a fact, earlier scan or `identities.json` gives the identity for that
   path, length and time. The install keeps in `identities.json` the identities
   its scans read while their files keep their length and time; a scan that
   cannot write them does not fail, and the next scan writes them. A fact gives
   the identity over an earlier scan of a path its writer has just written, and
   over `identities.json` at open: a file written twice within the volume's time
   resolution keeps its length and time, and the fact is the newer. A file rewritten
   outside toshokan to the same length and the same modification time, as a
   coarse clock or a tool that sets the time back allows, keeps the identity read
   before until its length or time moves: a binding can then take it for what it
   held. A precondition is never fooled, since it reads the identity at the
   check. After a commit, only the paths its file effects moved files from and to
   are listed again. Identities are read by the same rule, of every file whose
   identity is unread and now decides a binding, such as a file of the length of
   a file fact the commit wrote whose file is not at its path: that file may be
   the entity's, moved. A refresh scans again only the paths named by the file facts
   the entries it read changed, before and after, and the paths those entities
   were bound to, with the same identity reads. Until a scan lists such a path,
   as when the refresh stops first, no file there is bound or pinned, and the
   next refresh scans it. Such a path is found only under
   the spelling its directories list, since a volume that ignores case finds a
   file by any spelling of its name. A rescan, which the app asks for
   once a library opens and when something outside may have changed the folder,
   scans every file. Opening scans none: until a scan of every file succeeds,
   every library path is unknown. A scan that fails once a commit's entries are
   durable does not fail the commit. Until a scan of every file succeeds, the
   paths it would have scanned and what is under them are unknown: no file there
   is bound, reported or a candidate for a move. The entities bound there before
   the commit, and those whose file facts name such a path, are bound to no file
   and shown at their logged path as unscanned. Nothing about them is pinned, and
   each refresh scans every file until a scan of every file succeeds.
   A scan of every file lists one directory at a time and keeps what it listed,
   and the refresh that finishes it reports every directory it listed.
   A directory it cannot list is left out, as is every directory it has not
   reached once it has listed 1,000,000 entries: what is under them is unknown,
   as after a failed scan, and the scan reports them. So is a file whose identity
   it needs and cannot read for any reason but its absence: the file is unknown,
   as after a failed scan, and the scan reports it with those directories. A layout that hides leaves
   out every name that starts with a dot. Every listing leaves out symbolic
   links, special files and names that are not valid UTF-8. A view lists no
   file until a scan of every file has run.
   One that stops before every directory is listed binds nothing; the next goes
   on where it stopped, listing again each directory holding a path a commit or
   refresh scanned meanwhile, and each directory at or under such a path. A
   change outside the app to a directory already listed shows at the next scan
   of every file, or of its path.
2. An entity whose file fact names a path a file is at is bound to it: in sync
   when the file holds the fact's identity (or, without one, its length and
   time), else changed outside. Paths compare under the volume's rules for case
   and Unicode normalization. When several entities name one file, the lowest
   by id of those whose facts it holds gets it, in sync, and the others whose
   facts it holds are **joined** into that one, so readers of the same facts pick
   the same entity: two writers can each adopt one file before either sees the
   other's entity. A joined entity is bound to no file and is not missing; the
   scan reports it with the entity it joined, and a view shows it without a file
   until an intent joins it (see Intents). The others naming the file, or all when
   it holds no one's facts, go on to step 3.
3. An entity whose path holds nothing is bound to the one unbound file holding
   its identity: a move. With no such file it is missing; with several, or when
   one file could be several entities', nothing is bound and the candidates are
   reported.
4. Every other file is unbound, a copy of a bound entity's file included: it
   has no entity until an intent says something about it.

Every commit pins the moves this writer holds that its facts do not say yet: each
file in sync at another path, as a `pin` op in a `bind` entry. A path the
volume takes for the logged one, under its rules for case and Unicode
normalization, is not another. A new modification time alone is not logged; it
only spares a scan reading an identity. Conflicted file registers, changed files
and missing ones are left for the user. A scan writes nothing in the folder.

## Intents

An intent is one user action: fact changes and file effects committed together.
Opening, reading, refreshing and scanning write nothing in the folder; only
committing an intent (an undo, a redo or a settlement included), emptying the
trash, compaction and closing do.

A commit:

1. Turns the fact changes into ops against the merged state: a register write
   replaces every live write of the register, an add or remove every live write
   of its member, a delete observes every live field write of its entity, and
   the intent names the fork points it knows of the writers its ops name. Each
   change taken from an item names that item's run in `take`, so one intent can
   take from several runs. A file taken from an item is a `pin` op of the file
   the run names. A key the app did not declare, an entity that is not shown,
   or a taken file the last scan did not find at its path holding what the run
   says, or found bound to another entity, refuses the intent.
2. Turns the file changes into the steps below and their preconditions.
3. If the instance has no writer yet, checks the preconditions without writing
   and only then creates its writer, so a first intent a precondition refuses
   leaves nothing. One refused as too long leaves the writer's genesis entry, and
   the instance goes on writing as that writer.
4. Settles this writer's own unfinished effects (see Recovery).
5. Carries out the file effects under a pending record, then appends the intent
   with a `file` op for each entity's file as the folder shows it afterwards,
   and after it the `bind` entries that pin the moves found. The record is removed once all
   of them are durable.

Building an intent writes nothing, but draws the id of each entity it creates,
so one intent can create entities that name each other.

A commit whose file effects fail logs nothing and fails, leaving its record
open (see File effects and pending records).

No two file changes of one intent touch one path, or a path and one under it,
paths compared under the volume's rules for case and Unicode normalization;
such an intent is refused before anything is written. A move touches its source,
and its destination unless the volume takes the two for one name.

An intent may make a directory where nothing is, and remove one that holds no
file the library knows but those its earlier changes move out; anything else in
it keeps it, and the removal is then not logged. Neither touches a path, so one
intent can make a directory and move files into it, or move them out and remove
it; a directory made where another of its changes puts something, or under such
a path, refuses it. Each logs a `dir` op as the folder shows it afterwards. When
settling one of this writer's unfinished effects at step 4 fails, the commit
fails before its own intent is tried.

Otherwise a commit whose entries are durable succeeds, whatever fails after
them, and says which parts of the instance's own state lag it: `head.json` not
written, which the next commit, refresh or close writes again; or the scan after
it failed, which the next refresh repeats. Settling one
of this writer's own effects and settling another writer's effect commit in the
same way.

An intent may also adopt a library file no entity is bound to: a precondition on
its identity, no step, and a `pin` op giving the entity the file as it is.

An intent may join one entity into another, as the app does for each entity a
scan reports joined. It logs an `add` to the other of each member of each set
the one holds and the other does not, a `write` to the other of each register
the one holds a value of and the other holds none of, the one's shown value
where its writes conflict, and then the one's `delete`. A register both hold a
value of keeps the other's. Two writers joining the same entity converge: an
add of a member already held and a second delete of one entity change nothing
shown. The delete observes only the writes its writer saw, so a change to the
joined entity another writer made without seeing the join is a deletion
conflict. An intent that only joins entities is final (see Undo and redo).

An intent may save, rename or trash a library file no entity is bound to, by its
path, without making it an entity: a file nothing is said about stays without
one. Each is refused where an entity's file is, under the volume's rules. The
intent logs a `path` op for each such file as the folder shows it afterwards,
with where it was: `from` alone for a file trashed, `file` alone for a file
made, both for a file renamed or saved over. A commit reports each `path` op it
logged, an undo's and a redo's included.

A tree move is refused unless what it moves is still there: a directory at its
source, or a file where the last scan listed one, and, moving file by file, a
file at each path under it the last scan listed. A tree move logs a `tree` op,
and a `file` op for each entity whose file it moves; the files under it no
entity is bound to have no facts, and it logs no `path` op for them. A commit
reports each tree it moved, and each directory it was to remove that the
folder still holds: moving file by file, it moves only the files the last scan
listed, and a file put there since keeps its directory. Undo moves the tree back
as a tree move of its own, with a precondition on the identity of each entity's
file the move carried, in place of renaming each file back: a single `rename`
where directories rename, file by file elsewhere, which then removes each
directory under `to`, and `to`, left holding nothing. A file no entity is bound
to goes back with the tree as it is, and so does a file put in the tree after
the move. The move back logs a `tree` op, so a redo
moves the tree again.
A `path` op changes no fact; undo reads it to rename the file back,
bring its displaced bytes back from the trash, or trash the file it made.

## File effects and pending records

Nothing in a library path is overwritten or removed in place. An intent's file
effects are a list of steps, each of which moves one file or directory:

| Step                                      | Moves                                   |
| ----------------------------------------- | --------------------------------------- |
| `{"step":"to_trash","path":p,"item":n}`   | `p` to `trash/n`                        |
| `{"step":"place","staged":n,"path":p}`    | Staged file `n` to `p`                  |
| `{"step":"rename","from":p,"to":q}`       | `p` to `q`                              |
| `{"step":"from_trash","item":n,"path":p}` | `trash/n` to `p`                        |
| `{"step":"make_dir","path":p}`            | Creates directory `p`                   |
| `{"step":"remove_dir","path":p}`          | Removes `p` if it is an empty directory |

Paths in steps are library paths. `tmp/`, `trash/` and `pending/` are those of the
writer the record belongs to.

A save over a file is `to_trash` then `place`; a save where nothing is, `place`;
a trash, `to_trash`; a rename, `rename`, refused when something is at the
destination; restoring a trash item over a file, `to_trash` then `from_trash`. A
directory moves by one `rename` where the backend renames directories, otherwise
by a `make_dir` of its new name, one `rename` per file the last scan saw, then
`remove_dir` deepest first. A
`to_trash` whose path holds nothing when it runs is skipped, as there is nothing to
keep, and a `place` or `rename` makes the directories its destination needs, so a
save into a folder removed meanwhile makes it again.

A rename whose two paths the environment's `Names` take for one, as a change of
case alone where names fold case, finds the renamed file itself at the
destination on a volume that folds them too. Only an entry whose every name is
spelled exactly as the destination's then clashes, and the move is a `to_trash`
of the source and a `from_trash` to the destination, so that no step's source
and destination are one file, and the way-point is the writer's own. A directory
moves this way only where the backend renames directories.

Where the folder cannot rename files, each move copies instead, and directories
move file by file. The record says so with `"moves":"copy"`, and whoever carries
out its steps, the writer resuming them included, moves the same way.

A writer carries out an intent's steps in this order:

1. Create each new file in `tmp/` and fill it, then sync it and `tmp/`. A copy
   out of another writer's directory is made a chunk at a time. A file whose
   filling fails is removed.
2. Check every precondition: a path holds nothing, a file whose identity is the
   one the app expects, or, for a file the app has not read, a file of the length
   and modification time a scan listed, or any file; a directory satisfies none
   but the one a tree move starts from. An identity
   is read at the check, never taken from a scan. Check that every
   trash item a step restores is there, unless an earlier step of the intent
   puts it there. On failure, remove the staged files and
   write nothing more.
3. Write the pending record to `tmp/<nonce>.json`, sync it, and rename it to
   `pending/<nonce>.json`. A record whose moves copy is created at
   `pending/<nonce>.json` and synced with its directory.
4. Carry out the steps in order. A move first creates the destination's
   directory, checks that nothing is at the destination, renames without
   replacing, syncs the destination's directory and then the source's, so a
   source's name is gone only once the destination's is durable. Consecutive
   `place` steps into one directory are moved together, then that directory and
   `tmp/` are synced once for all of them; a crash between the two syncs leaves
   staged files beside the placed ones, and settling removes them.

   A move that copies first creates the marker `pending/<nonce>.<i>`, where `i`
   is the step's index, and syncs `pending/`. It then creates the destination,
   refused when something is there, copies the source into it a chunk at a time,
   checks that it holds the source's length, syncs it and its directory, and
   only then removes the source and syncs the source's directory. Once the copy
   is written, and again just before the source is removed, the source must have
   the length and modification time it had when the copy began; otherwise the
   source stays, the copy is removed, and the step fails. A copy that fails
   removes what it wrote. Each step runs alone.
5. Append the intent's entry.
6. Remove the pending record, after its markers, once the entry is durable. Its
   entry is logged, so a record a crash or a failed removal leaves is spent: no
   reader that has the entry reports it. The writer removes it before its next
   write, which fails while it cannot, and when it closes.

Where the folder's renames refuse an existing destination themselves, as
`renameat2` with `RENAME_NOREPLACE`, `renamex_np` with `RENAME_EXCL` and
`MoveFileExW` without `MOVEFILE_REPLACE_EXISTING` do on volumes that support
them, a move never replaces anything. Elsewhere a rename may replace, and the
check before it is the only guard: a file another program makes at the
destination between the check and the rename is replaced, and its bytes leave
the folder. A move that copies creates its destination, which replaces nothing
where the backend's creates refuse an existing file in the same step; a
browser's do not (see Browsers). Opening says which kind of folder it is.

If a step fails, the rest are not tried and nothing is logged: the record stays
open, and the next write, or the next open, settles it forward from what the
folder shows. If that fails, the writer stops writing until the library is
opened again; where what the rest moves is gone, it gives the record up to be
settled with consent (see Recovery). Each library path holds its old bytes, nothing, or its new bytes, and
while it holds nothing a pending record names it. Where moves copy, a library
path may also hold a copy of its new bytes cut short while a pending record
names it: no longer than them, holding their start or, where a crash zeroed or
tore bytes the volume had not yet synced, anything else.

Removing a record first removes its markers and syncs `pending/`.

A pending record is a JSON object:

```json
{"writer":"<w>","entry":<entry>,"label":"Save","steps":[<step>,…],"files":[{"entity":"<e>","path":"a/b.syx","done_after":2}]}
```

`entry` is the intent's entry as planned, without its `file`, `path`, `tree`
and `dir` ops and `displaced`, whose `prev` is the writer's head when the record
was written.
`"moves":"copy"`, omitted otherwise, says the steps move files by copying.
`"final":true`, omitted otherwise, says the intent is final (see Undo and redo),
as a settlement is, so that it is no step of undo when a later session logs it.
`files` says where each entity's file is once the first `done_after` steps are
done; a `path` of `null` is no file. `"pin":true` marks a file the intent adopts
as it is, logged as a `pin` op rather than a `file` op. `replaces`, omitted when
empty, holds the references the entity's `file` or `pin` op replaces: the writes
of its file register the intent's writer knew of when it planned it. A record
without `replaces` replaces nothing, so its file ops stand beside every write of
the register they change. `unbound`, `trees` and `dirs`, each omitted when empty,
say the same of what no entity owns, for the `path`, `tree` and `dir` ops:
`unbound` holds `{"from":…,"path":…,"done_after":n}` for a file no entity is
bound to, with `from` or `path` omitted for no file; `trees` holds
`{"from":…,"to":…,"done_after":n}` for a tree moved whole; and `dirs` holds
`{"path":…,"made":true,"done_after":n}` for a directory made, or removed where
`made` is `false`. `held`, omitted when empty, holds `[p,expect]` for each
`to_trash` source `p` whose contents the intent checked: `{"holds":"<identity>"}`,
or `{"listed":{"len":n,"modified":t}}` with `t` `null` for no time. A reader
reads at most 16 MiB of a record.

### Recovery

A record is closed by an entry that names it: its writer's intent whose `record`
is the record's nonce, or another writer's `settle` entry. A tip that folded such
an entry still names it, and so does an entry in a run, in a gap or dismissed,
whose effect is that branch's. A record no entry names is open, wherever its
writer's log has gone since; one that is closed is spent, waiting only for its
writer to remove it.

A record is ignored when it does not decode, names another writer, names a path
that is not a library path, or its `prev` is not held: it is not a tip's head, an entry a tip's certificates
name, or numbered below a tip's head, and no entry merged, in a run or in a gap,
has it. A record of another writer that cannot be read for any reason but its
absence is reported and read again at the next refresh, and until then no
orphan is reported or settled; one of this writer's fails the open or refresh,
and the trash and staging it may name are left as they are. Opening reports the
records it ignores, and the open records of other writers as orphans. Each
refresh reads every writer's pending records again and reports the orphans as
they stand then, since a writer elsewhere may have begun or finished an effect
meanwhile, each with whether it is the install's own and live as the pool and
its locks say then, and with a hash of what the folder shows of its paths and
markers, which moves while a writer carries the effect out.

Opening finds this writer's open records whose `prev` is its head, and says how
settling each will end. The writer settles them before it checks its next
intent, undo, redo or settlement against the view: it finds how far the steps
got by checking them from the last, and the first that shows done ends the done
prefix. `to_trash` shows done when its trash item exists; any other move when
its destination exists and its source does not, or both hold the same bytes. A
`place` whose staged file is in the trash under its own nonce is not done. A
path counts as there only under the spelling its directories list, since a
volume that ignores case finds the destination of a change of case by its
source's name. When
the last done step's source still holds the same bytes as its destination, the
source is removed. The remaining steps are carried out, the planned entry is
appended with what the steps did, and the record is removed. A record is
settled while it reads as the writer found or wrote it, whatever members it has
that the writer does not know; one the folder does not hold, or holds cut short,
never landed and is dropped. Then every `pending/<nonce>.json` opening or
settling found that is a record created in place and cut short before its bytes
landed, before any of its steps ran, is removed: one that is empty or, in a
folder that syncs, one of at most 16 MiB that holds a zero byte or is the start
of a JSON value. Any other record that does not decode stays, and while one
stays nothing in `tmp/` is removed, since it may place any of it. Otherwise
every file in `tmp/` that no record in `pending/` places is removed.

A record whose remaining steps move something the folder no longer holds, so
that no retry could carry them out, is not resumed: the writer stops writing, as
when its history cannot go on, and the record is an open record of another writer
to settle with the user's consent, below. Settling it changes nothing until then.
What is gone is judged from one look at the folder, so a file only briefly absent,
as where a tool replaces it by removing it first, gives the record up as well. A
`to_trash` whose path holds nothing is skipped rather than gone. A record is given
up the same way when a remaining `to_trash` finds its source holding other than
its `held` contents, as when another writer saved over the file since, so that
newer bytes never go to the trash on the strength of an old check.

Settling twice ends as settling once.

The steps of a record whose moves copy are found from its markers instead,
since a later step may put bytes back where an earlier one copied them from.
Without a marker, no step is done. Otherwise every step before the last marked
one is done, and that one is done when its source is gone and its destination
is there, unless it is a `place` whose staged file is in the trash. When its
source and destination hold the same bytes, it is done and the source is
removed. Otherwise the destination is that step's copy cut short, since the
marker was synced before the destination was created and the create refuses an
existing file, while it holds the start of the source's bytes or, in a folder
that syncs, where a crash zeroes or tears bytes not yet synced, anything no
longer than them. A copy holding the start of the source's bytes is removed;
any other is set aside in the writer's trash, under a name derived from the
record and step that no entry names, so emptying the trash never removes it.
Then the step is carried out again. Any other destination stays, and the step
stops there.

The destination may instead be another program's file: one made after the
marker was synced and before the writer's create, which then fails, or one put
there between the crash and the next open, such as a file restored from a
backup. Recovery cannot tell such a file from its own copy cut short. It keeps
its bytes all the same: it sets the file aside in the trash, or removes it only
where it holds the start of the source's bytes, which the source still holds.

An open record of any other writer may belong to a live writer elsewhere, or to
this install's own writer from before its local root was lost; a reader cannot
tell them apart. It reports the record, and settles it only when the user asks,
or, for a record that is `ours` (see Writers), when the app asks without asking
the user, as an intent of its own: to finish, it carries out the remaining steps, skipping
a `to_trash` whose path holds nothing, and logs the facts the record planned and
its file ops with the record's `replaces`; to
roll back, it reverses the done ones, skipping a file it would trash that is not
there; to dismiss, it changes no file. Finishing or rolling back is refused, and
changes nothing, where a step would move what the folder no longer holds.
Finishing checks each `to_trash` source the folder holds against its `held`
contents as a precondition, and is refused where it holds anything else; so is
finishing or rolling back a step that puts a file where the folder holds one no
earlier step moves away. It finds
how far the steps got by checking them from the last, whichever way they move,
since its own settling may have carried them further or reversed some; a
`to_trash` of a record whose moves copy shows done as any other move does. A
copy cut short, as above, at the destination of the record's last marked step
makes that step the one in progress and goes to the settler's trash first, as
does a library path the last done step left beside its destination. It
copies files out of the other writer's `tmp/` and `trash/` rather than moving
them, a directory there file by file, making each directory under it, and moves
a library file it displaces into its own trash. A `from_trash`
of an item a `to_trash` it carried out itself put aside takes the file from its
own trash, where that step put it, and so does rolling back a `to_trash` whose
`from_trash` it reversed. Once its steps are done, it appends a `settle` entry
naming the writer and record, after which no reader reports the record. If a
step fails, the settlement fails as an intent does: the settler's own record
stays open, and the other writer's record stays open until the settlement is
made again, which finds from the folder what remains. Only the record's writer removes it.

A record written where directories rename may move a tree by one `rename`, or,
for a change of case alone, by a `to_trash` and a `from_trash`. A reader whose
folder cannot rename directories refuses to finish or roll back a step that
would rename a directory or move one to the trash, and changes nothing; it can
still dismiss the record, and a folder that renames directories can settle it.

## Undo and redo

Each instance undoes only the intents it committed since it opened, most recent
first, by committing a compensating intent. A redo reverses the latest undo
while the instance has committed nothing else since. Undo history is not logged:
it ends when the instance closes. Each intent is kept with what it replaced, as
the merged state showed it when it committed, and with where the binding found
each file it changed or whose entity it deleted, so a file moved outside the app
that no commit had pinned yet goes back where it was moved to. An intent that only
dismisses runs or joins entities, or settles another writer's record, is final: it
is not kept and leaves redo as it was, so an undo after it reverses the intent
before it. Undo takes what a join or a settlement wrote for another writer's.

- A register write is reversed by writing back the latest value it replaced,
  another writer's included; a clear where there was none.
- An add is reversed by a remove, a remove by an add.
- A create is reversed by a delete, and a delete by a create replacing it, with
  the register values, members and file the delete ended.
- A `file` op is reversed by putting back what it changed: the displaced bytes
  from the trash, under a precondition on what the path holds now; the old name;
  or the trash, for a file the intent added. A save over a file that was gone
  before it could go to the trash displaced nothing, so its undo leaves the saved
  file.
- `pin` ops are left alone, so undoing an adoption leaves the file.
- A `dir` op is reversed by removing the directory it made or making the one it
  removed.

An intent's file changes are undone in the reverse of the order it made them,
except that a directory it removed is made again before anything else, and one it
made is removed after everything else: the log keeps tree moves, then directories,
then files, whatever order the intent made them in.

An undo or redo is refused, naming the writer and entry, where another writer
changed the same thing since: a register no longer holds only what the intent
left, another writer added or removed the same value, a delete of the entity is
live, or another writer wrote to an entity being uncreated. A file register holds
what the intent left while every live write of it names the same path and
identity, whatever modification time it gives, since putting a file back where
moves copy gives it a new one. It is refused when the trash no longer holds the
bytes it needs.

## Trash

`trash/<nonce>` holds bytes an intent of the writer displaced, or new bytes an
intent staged and could not place. An item is listed with the entry whose
`displaced` names it. Only its writer removes an item, and only by emptying,
which first removes the writer's spent records, then every item but those the
app keeps and those a pending record of the writer still in the folder names,
since settling the record may move them. While a pending record of the writer
does not read, emptying removes nothing and names the record. A file in
`pending/` named as neither a record nor a marker, as one another program leaves,
is no record and holds nothing. An
item no entry names is never emptied. Emptying removes the items, then syncs the
trash once; an item whose removal a crash undid is removed by the next emptying.

Emptying a trash is the only way bytes leave the folder.

A writer that stops while its instance runs, other than over a fork of its
history, empties its trash before it retires, since stopping ends this session's
undo: it reads its directory again, removes its records an entry closed, as one
whose write landed though it was reported failed, and then every item no record
of the writer still in the folder names. Its open records become orphans and
keep what they name. A writer whose history forked keeps its trash, since
another instance may still write as it, and so does a writer an instance retires
as it opens. Where the folder cannot be written, the writer retires without
emptying; where emptying fails, or a record that does not read holds it, the
writer retires all the same, and the instance reports why (`Lag::Trash`).

An instance lists the trash of every writer whose directory its local root
holds: its writer's and those of the writers it retired. A retired writer's
items stay where they are, since no instance writes as that writer again; undo
reaches only the items of this session's writer.

## Compaction

Only a snapshot's writer compacts: at close when it has written since its last
snapshot, every 512 of its own entries, and when the app asks. It folds from
what the folder holds as it reads it now. It confirms that a file in its
directory holds its last entry, as before an append; if none does, it writes
nothing and a new writer takes over. When a snapshot in its directory already
has that head, there is nothing to compact: it writes nothing, deletes as below
only what that snapshot lets it, which a compaction cut short left, and looks
first at that snapshot when it next confirms the entry. Otherwise it folds the
entries after its previous snapshot's head into that snapshot's state (see
[Snapshots](format.md#snapshots)). It certifies every segment it knows its
chain was written to by the part of that segment on its chain, the snapshots of
its directory on its chain by their certificates, and every other entry it folds
by hash, and writes the snapshot. It knows a segment from its previous
snapshot, from the files it read or wrote since, and from the files in its
directory now. A segment file that ends before a certificate of the previous snapshot with its first line
reaches, as a dropped version does, gets no certificate of its own. A snapshot
longer than a reader reads (see Reading) is refused before anything is written,
so the log it would fold stays, and the instance compacts no more on its own
until it opens again.

Where the folder renames files, it writes the snapshot to `tmp/` under its
nonce, syncs it, renames it into place and syncs both directories; a staged
snapshot a crash leaves is removed with the other files of `tmp/` (see
Recovery). Elsewhere it writes the snapshot in place and syncs it and its
directory, and a crash can tear it, leaving a file reported as unreadable. The
writer's next compaction there deletes a `snapshot-` file of its directory that
ends, but for a zero-filled tail, inside its JSON value, with what that
compaction supersedes; a whole snapshot it cannot read, as a newer build writes,
stays.

It then confirms that the folder holds the snapshot. Only then does it delete
every segment in its directory that ends with a seal marker and every line of
which the new snapshot certifies, and every snapshot whose head the new one
certifies. A segment without a seal marker, such as one left open by a crash or
a copy, is never deleted, and neither is a segment holding an entry off the
chain. Just before deleting each file it confirms that the new snapshot is still
in the folder, and that the file still has the length it read and, for a
segment, ends with its seal marker; a file that changed is left.

## Bounded retention

The folder keeps, for the life of a writer:

- one snapshot, whose contribution is bounded by the fields its writer wrote
  (see [Snapshots](format.md#snapshots)), but for two parts that grow with use:
  a certificate per segment it ever wrote, about 90 bytes each, where a segment
  ends at each close of a process that appended, each compaction and each MiB;
  and a pair in `settled`, about 70 bytes, per commit that moved files and per
  settlement;
- the segments written since that snapshot, and one segment per process that
  crashed or was copied;
- trash items until the writer empties them (see Trash);
- a spent pending record until the writer's next write or close;
- once it will never run again, its pending records, settled or not, with the
  trash items they name.

Nothing else in a snapshot grows with history, and nothing in it grows with the
library's size. What a retired writer leaves stays for good, since no instance
writes as it again. The local root keeps one directory per writer the install
has used.

## Backends

A backend declares, for each of the folder and the local root, what it can do,
and toshokan plans from that alone, never from a request that failed:

| Capability    | Meaning                                                        |
| ------------- | -------------------------------------------------------------- |
| `append`      | Bytes can be added to the end of a file                        |
| `rename_file` | A file can be renamed atomically                               |
| `rename_dir`  | A directory can be renamed atomically, with everything in it   |
| `no_replace`  | A rename refuses an existing destination in the same step      |
| `fsync`       | A sync makes completed requests durable                        |

Without `rename_dir`, a directory moves file by file, and without `rename_file`
each move copies and then removes its source (see File effects). Without
`append`, a library opens read-only. Without `fsync`, a sync is answered at once,
and a completed request is as durable as the backend makes it (see Durability).
The local root must rename files. Where a crash can tear a rename, as on FAT and
exFAT, a crash while a file of the local root is replaced can lose that file, as
losing the local root would.

The native backend tells a volume by the type of file system it reports, read at
the root or, before the root exists, at the nearest directory above it. It
declares `rename_file` and `rename_dir` except where the file system keeps no
journal of its names, so that a crash can keep a rename's removal of the old
name without the new one, and where it cannot tell: FAT, exFAT, NTFS, HFS, HFS+
and UDF on Linux; FAT, exFAT, NTFS and UDF on Apple systems; FAT, exFAT and UDF
on Windows; every FUSE file system, which hides what it stores to; and a volume
whose type cannot be read. There every move copies. Whether renames are atomic
is judged by the type alone: ext2, and ext4 made without a journal, report
ext4's type and are taken to rename atomically, as is any file system not named
here. It declares `no_replace` where renames also refuse an existing
destination (see File effects), which on Apple systems it reads from the
volume's capabilities. On Windows it syncs a file through a handle that may
write and a directory through one opened for backup semantics, and its renames
return once they are on the disk. Where a volume refuses to flush a directory, a
file's flush and a rename written through still flush its log through every
earlier change of names. Reported by public documentation; not confirmed on
hardware. On Apple systems it asks for a full sync, and takes a plain one where
the volume refuses it, as network volumes do. Reported by public documentation;
not confirmed on hardware. On Linux and Apple systems, a volume that cannot sync
a directory has each sync of one taken as done. A request another program holds
the file of for a moment, as antivirus does after a write, is tried again for
about half a second, and a file's sync for about five seconds, since its bytes
have landed.

### Browsers

The folder is a directory of the origin private file system or a folder the
user picked; the local root is always a directory of the origin private file
system.

| Folder                                      | `append` | `rename_file` | `rename_dir` | `no_replace` | `fsync` |
| ------------------------------------------- | -------- | ------------- | ------------ | ------------ | ------- |
| Private file system, Chromium               | yes      | yes           | no           | no           | yes     |
| Private file system, Firefox and WebKit     | yes      | yes           | no           | no           | no      |
| Picked folder, Chrome                       | yes      | yes           | no           | no           | no      |
| Picked folder, other Chromium browsers      | yes      | no            | no           | no           | no      |

- **Private file system.** Files are read through `getFile()`, which takes no
  lock, and written, appended to and flushed through sync access handles, opened
  for one request each. Chromium's `flush()` takes a fraction of a millisecond,
  as a write that reaches the disk does, and Firefox's and WebKit's take
  microseconds, so only Chromium declares `fsync`. That Chromium's reaches the
  disk is inferred from its cost. A directory's names live in the browser's own
  database, which toshokan cannot sync, so a sync of a directory returns at
  once, and a name made or removed is only as durable as that database keeps it.
  Whether such a name survives the machine stopping has not been tested.
  Chromium cannot rename a directory there, so no browser declares `rename_dir`.
  A write the origin's quota refuses fails with `NoSpace`: Chromium throws
  `QuotaExceededError`, and Firefox writes fewer bytes than it was given.
- **Picked folder.** Files are read through `getFile()` and written through
  writable streams, which write a copy and put it in place at `close()`: that is
  the moment a write lands, and nothing is synced beyond it. An append copies
  the whole file, which the 1 MiB bound on segments keeps small. The writes that
  fill one file share one stream, kept open while other files are read. A file
  created with bytes is named first and filled at its stream's close, so a tab
  stopped between leaves it empty. Before putting a file in place, Chromium
  checks it with Safe Browsing, in full unless its type is one Chromium samples,
  such as `.txt` and `.json`, the types of toshokan's own files; its published
  file-type policy still checks some closes of a sampled type in full. A saved
  library file is staged under its own extension, so a type Chromium does not
  sample pays the full check. A trash item has no extension, so where moves
  copy, each file moved into the trash pays the full check. Renaming there was
  tested only in Chrome, which renames, and Brave, which refuses. Every other
  browser is taken to refuse: a picked folder declares `rename_file` only where
  `navigator.userAgentData` names Google Chrome and `navigator.brave` is absent.
  A rename the browser refuses although the folder declares it fails with
  `Unsupported`.
- **Paths.** In Firefox, each step of a path and each file's handle grows slow
  once the origin holds many files.
- **Renames** replace a file at the destination in every browser, so none
  declares `no_replace`. **Creates** check that nothing is there and then make
  the file, which opens one already there, so a file another program makes
  between the two is replaced by a copy's bytes, or removed when the copy finds
  it too long, as a rename replaces one.
- **Swap files.** While a writable stream is open, Chromium keeps its bytes in
  `<name>.crswap` beside the file. A tab stopped meanwhile can leave that file
  behind, which is inferred and has not been seen. toshokan reads, scans and
  reports no file whose name ends in `.crswap`, wherever it is, and refuses such
  a name as a library path.
- **Times.** Chromium and Firefox report a file's last change in milliseconds,
  WebKit in whole seconds: two changes that keep a file's length within that
  time look alike.
- **Locks** are Web Locks named `toshokan:/<local root path>/<name>`, held by
  the executor and released when its worker ends.
- **Hints.** A tab that commits announces its newest entry on the
  `BroadcastChannel` `toshokan:<library>`, and the library's other tabs refresh.
  Only Chromium has a `FileSystemObserver`, through which a page can watch its
  folder.

## Guarantees

**Kept.**

- Readers that see the same files merge and show the same entries, at once,
  whatever order the files came in; after a restore, what the restored folder
  says.
- Every kept entry is merged, shown as a run, dismissed by the user, or held back
  after an entry a restore or a dropped version took.
- An entry no fork precedes and that follows nothing lost is merged.
- No two branches of a writer are ever merged together.
- A reader finds every fork point from the files, whichever branch's files it
  sees first, but for a fork inside a segment.
- A run stays shown until it is dismissed or its files go.
- A step by one branch of a forked writer changes no merged value, except on a
  field shown as a forgotten basis.
- A replaced value or removed member never comes back without a conflict, also
  while sync is in flight, but as Not kept says of a fork its writer did not
  know of: a write a reference covers stays dead, and a reference the files
  contradict shows what it replaced only beside the current value.
- A delete's references are kept for good, so an edit made long after, by an
  instance that had not read the delete, is still a deletion conflict.
- No offline writer's work stops counting, however long it was away.
- Chain order, single-writer directories, no bytes leaving the folder except by
  emptying a trash, total recovery, opening writing nothing.

**Not kept.**

- A fork's branches are not merged side by side: each branch after the fork
  point is a run for the user, however briefly both copies of a local root ran.
- What a reader shows does not only grow while sync delivers files out of
  order: facts can disappear and come back.
- A branch that compacts past a pre-fork value it replaced takes that value out
  of the merge, and the field shows as a forgotten basis.
- Where several tips part inside a segment, a pre-fork entry with no effect left
  can show as a run, and an own write that may be pre-fork as a forgotten basis.
- Reviving a deleted entity brings back only the concurrent edit.
- A reader may show a field empty, or as a conflict, until the write that
  replaced the value it showed arrives.
- A copy of a segment that diverged and stops short, unsealed, hides its write
  to a field the other copy wrote at a higher number; a copy one line longer is
  a run, and shows it.
- A writer that replaced two writes of another writer to a field before it saw
  a fork of that writer keeps a reference only to the later one. Once it
  compacts, a reader that finds the fork between the two shows the earlier
  write's value again.
- A snapshot lost while a newer segment survives leaves that segment held back
  as a gap until the writer's next snapshot reaches it.
