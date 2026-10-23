# toshokan format and protocol

This document specifies everything toshokan writes and how it reads, merges and
changes it, precisely enough for a second implementation to share a library with
this one. The format is unstable while toshokan is a proof of concept.

All text is UTF-8. All hexadecimal is lowercase. JSON is written without
insignificant whitespace.

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

A backend also says whether it can append, rename a file, rename a directory,
and rename without replacing. toshokan plans from what it says, never from a
request that failed. A folder that cannot append is opened read-only. The local
root must rename files. Where a crash can tear a rename, as on FAT and exFAT, a
crash while a file of the local root is replaced can lose that file, as losing
the local root would.

## Layout

The app chooses a library folder and the name of a root directory inside it,
such as `.drawbar`. In the folder, toshokan writes only under that root, and in
the library's files only as an intent's file effects ask. Every path in this
table is relative to the root.

| Path                                  | Contents                                |
| ------------------------------------- | --------------------------------------- |
| `writers.2/<w>/<segment>.txt`         | A segment of writer `w`'s log           |
| `writers.2/<w>/snapshot-<nonce>.json` | A snapshot of writer `w`'s log          |
| `writers.2/<w>/pending/<nonce>.json`  | The record of an unfinished file effect |
| `writers.2/<w>/pending/<nonce>.<i>`   | Empty: step `i` of `<nonce>` has begun  |
| `writers.2/<w>/trash/<nonce>`         | Bytes an intent of `w` displaced        |
| `writers.2/<w>/tmp/<nonce>.<ext>`     | A file `w` is staging                   |

Only writer `w` creates, appends to, renames or removes anything under
`writers.2/<w>/`. Others only make and sync the directories `writers.2/` and the
root. There is no file at the level of the library. Nothing reads an earlier
format's `writers/`.

A staged file is named by its nonce and the extension of the file it becomes:
what follows the last `.` of that file's name, if anything does, and if the name
stays within 255 bytes; otherwise by its nonce alone. A trash item is named by
its nonce alone, so a step names it without knowing where its bytes came from.

Names of segments and snapshots are advisory. A reader reads every file directly
in `writers.2/<w>/`, whatever its name, by its contents, so a sync client's
conflicted copy is read like any other file. A browser's swap file is the
exception (see Browsers).

A **library path** is a path in the folder outside the root, other than the
folder itself and a browser's swap file. Only library paths are the user's
files.

Each install keeps, per library, a local root of its own that is never synced:

| Path                             | Contents                                     |
| -------------------------------- | -------------------------------------------- |
| `<genesis>/head.json`            | The last entry this writer wrote             |
| `<genesis>/view.bin`             | The cached view                              |
| `<genesis>/view.log`             | What the cached view gained since            |
| `<genesis>/drafts/<entity>.json` | An unsaved edit                              |
| `<genesis>/lock`                 | Held while an instance writes as this writer |
| `<genesis>/retired`              | Empty; the writer is never written again     |
| `identities.json`                | Identities the install's scans read          |

`<genesis>` is the hash of the writer's genesis entry. The directory is created
only after that entry is durable in the folder, so the directories of the local
root are the install's pool of writers.

`head.json` is `{"writer":"<writer id>","head":"<entry hash>"}`, and
`identities.json` is
`[["<library path>",<length>,<modification time or null>,"<identity>"],…]`.
A file in the local root that is replaced, such as `view.bin` or a draft, is
first written beside it as `<name>.next` and synced; then `<name>` is removed
and `<name>.next` renamed to it. A reader takes `<name>`, or `<name>.next` when
`<name>` is missing. Every `head.json` of one writer has the same length, so a
`head.json` with the new record's length is written over in place and synced;
any other is replaced as above. A crash can tear that write. What it leaves is
the old or the new record whole, bytes that do not read, which leave the writer
out of the pool, or a record naming an entry no log holds, which retires the
writer (see Writers).

## Identifiers and clocks

Every identifier is 128 bits written as 32 hexadecimal digits.

| Name       | Meaning                                                              |
| ---------- | -------------------------------------------------------------------- |
| writer id  | A writer, random                                                     |
| entity id  | An entity, random                                                    |
| segment    | A segment's name, random                                             |
| nonce      | A snapshot, pending record, trash item or staged file's name, random |
| entry hash | An entry's id, link and checksum                                     |
| identity   | What the app's identity function says a file holds                   |

The identity function is the app's: a cheap fingerprint of a file's contents
from a few ranges of its bytes. toshokan compares identities and never computes
one itself.

A clock reading is the JSON array `[wall_ms, counter]`: milliseconds since the
Unix epoch and a counter within the millisecond, a hybrid logical clock. A
writer's next reading is later than every reading it has seen up to one day past
its machine's clock; a reading further ahead is shown but not followed, so one
wrong or forged clock cannot hold back the others. A counter that runs out moves
the reading to the next millisecond. Readings order writes for display only, by
reading and then by writer id.

## Segments

A segment is a sequence of lines. Each line is:

```text
<json> TAB <hash> LF
```

`<json>` is one entry, a JSON object, with no tab or newline. Its `prev` member is
the hash of the entry before it in the writer's chain, or 32 zeros for the
writer's genesis entry. `<hash>` is the first 16 bytes of the BLAKE3 hash of the
16 bytes of `prev` followed by the bytes of `<json>`. The hash is the entry's id.

For example, the JSON `{"prev":"0…0"}` makes the line:

```text
{"prev":"00000000000000000000000000000000"}	21fbde4eb554d0b73a2edf6821643616
```

A line longer than 1 MiB, a line whose hash does not match, a final line without
its LF, or a line starting with a zero byte ends the readable part of the file
for now. Every line before it counts; a later read of the same file may get
further.

A process that closes a segment ends it with the **seal marker**: `sealed`, TAB,
the hash of the segment's last line, LF. The marker is not a line, and nothing
follows it. A segment ends with a seal marker when its last bytes are exactly
the marker naming its last line; any other bytes after the last line end its
readable part. The marker's last 34 bytes are those of the line it names.

## Entries

An entry is an object whose first members are:

| Member | Value                                           |
| ------ | ----------------------------------------------- |
| `prev` | The hash of the entry before it                 |
| `n`    | Its number in its writer's chain                |
| `at`   | Its clock reading                               |
| `kind` | `"genesis"`, `"intent"`, `"settle"` or `"bind"` |

followed by the members of its kind. A genesis entry is numbered 0, and every
other entry its predecessor's number plus one. A reader ignores members it does
not know, at any depth, and keeps the line and the whole entry (see Merging), so
they survive. An entry of another kind, without `kind`, or whose members do not
decode, is kept and merged as unknown; so is a line whose JSON has no readable
`n` or `at`, which still links the chain but is never placed.

An entry names another as `["<writer>",<n>,"<hash>"]`, and one of its own
writer's as `[<n>,"<hash>"]`.

A **genesis** entry starts a writer's chain, with `prev` all zeros:

| Member   | Value                              |
| -------- | ---------------------------------- |
| `writer` | The writer's id                    |
| `label`  | The name other writers show for it |

A genesis entry after another entry, or numbered other than 0, is unknown.

An **intent** entry logs one committed intent, the unit of commit, undo and
attribution:

| Member      | Value                                                            |
| ----------- | ---------------------------------------------------------------- |
| `label`     | What the user did                                                |
| `ops`       | The fact changes, in order                                       |
| `displaced` | Bytes it moved into the writer's trash; omitted if none          |
| `forks`     | The fork points its writer knew of, `["<writer>",<n>]`; omitted if none |

Each op is an object whose `op` member names it:

| `op`      | Members                                          | Meaning                                      |
| --------- | ------------------------------------------------ | -------------------------------------------- |
| `create`  | `entity`, `replaces`, `take`?                    | The entity exists                            |
| `delete`  | `entity`, `replaces`, `observed`, `take`?        | The entity is deleted                        |
| `write`   | `entity`, `key`, `value`, `replaces`, `take`?    | A register holds `value`; cleared without it |
| `add`     | `entity`, `key`, `value`, `replaces`, `take`?    | A set gains `value`                          |
| `remove`  | `entity`, `key`, `value`, `tags`, `take`?        | A set loses `value`                          |
| `file`    | `entity`, `file`, `replaces`, `take`?            | A file effect left the entity's file here; none without `file` |
| `pin`     | `entity`, `file`, `replaces`                     | A reader bound the entity's file here        |
| `dismiss` | `runs`                                           | The user dismissed these runs                |

`replaces` and `tags` are arrays of references to the writes of the same field
the writer saw live: of a register, of a set's member, or of the file register.
`observed` names the live field writes of the entity a delete saw, each as
`{"key","at"}` for a register, `{"key","value","at"}` for a set's member or
`{"file":true,"at"}` for the file register, with `at` the reference. A `value`
is the app's JSON, kept byte for byte; `"value":null` writes `null`. A `file` is
`{"path","identity","len"}` with an optional `modified`, the backend's
modification time, compared only for equality. Each member of `displaced` is
`{"item","from","identity","len"}`: the trash item's nonce, the library path the
bytes left, their identity and length. An op of another name, or missing a
member, is kept and unknown.

A run, which `take` and `runs` name, is `{"writer","from","first"?,"last","tip"?}`:
the writer's entries numbered from `from` to `last` on `last`'s chain, the first
by its hash; a run a snapshot holds is named by that snapshot's head as `tip`,
and its first entry by number alone (see Reading).

For example:

```json
{"prev":"…02","n":3,"at":[3,0],"kind":"intent","label":"Tag","ops":[{"op":"add","entity":"…0e","key":"tags","value":"x","replaces":[]},{"op":"write","entity":"…0e","key":"origin","replaces":[["…0b",4,"…01"]]}]}
```

A **settle** entry records that this writer settled another writer's unfinished
effect with the user's consent:

| Member    | Value                                          |
| --------- | ---------------------------------------------- |
| `writer`  | The writer whose pending record it settles     |
| `record`  | The record's nonce                             |
| `outcome` | `"finished"`, `"rolled-back"` or `"dismissed"` |

A **bind** entry pins moves this writer's reader found (see Binding), after the
intent it committed with:

| Member | Value         |
| ------ | ------------- |
| `ops`  | Its `pin` ops |

Its ops are merged as an intent's are, and it is never undone. A commit spreads
its pins over as many `bind` entries as keep each line within the limit, so any
number of moves can be pinned.

Nothing derived is logged: views, bindings a reader has not pinned, and the
merged state exist only in readers and in snapshots.

## Merging

A reader merges what each writer still contributes: its tip snapshots' state
and the entries it places after them (see Reading). The merge is a function of
those, so readers holding the same files and memories show the same, whatever
order the files arrived in. Every write is named by its writer, number and hash.

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

Killing below `n` is sound because a writer's write replaced every earlier
write of its own it saw, a writer always sees its own, and no two branches of a
writer are merged.

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

## Snapshots

`snapshot-<nonce>.json` is one JSON object, without a final newline:

| Member      | Value                                              |
| ----------- | -------------------------------------------------- |
| `writer`    | The writer's id                                    |
| `label`     | The writer's label                                 |
| `at`        | The clock reading of the last folded entry         |
| `genesis`   | The hash of the writer's genesis entry             |
| `n`, `head` | The number and hash of the last folded entry       |
| `certified` | What lets a reader recognize every folded entry    |
| `state`     | What the writer contributes                        |

Members a reader does not know are kept, and a compaction carries those of the
snapshot it folds into the new one.

`certified` is `{"segments"?,"heads"?,"hashes"?}`. Each member of `segments` is
`[[<n>,"<hash>"],[<n>,"<hash>"]]`: a segment the chain was written to, by its
first line, and the last of its lines the snapshot folds. `heads` names the head
of every snapshot of its directory on its chain, and `hashes` every other folded
entry and the last entry of each dismissal of the writer's own runs on its chain.
A reader refuses a snapshot with a certificate whose last line comes before its
first, or that names an entry numbered past its head.

`state` holds, per entity, records that each name a field: no member for
existence, `key` for a register, `key` and `value` for a set's member, `"file":true`
for the file register.

```text
{"entities":{"<entity>":{
   "writes":[{field…,"at":[n,"h"],"stamp","first","set"?,"off"?,"pin"?}],
   "refs":[{field…,"above"?,"own":[n,"h"],"target":["w",n,"h"]}],
   "killed":[{field…,"own":[n,"h"]}],
   "takes":[{field…,"writer","own":[n,"h"],"below"}]}},
 "dismissals":[{"own":[n,"h"],"run":{…}}],
 "trash":[{"item","entry","at","from","identity","len"}],
 "settled":[["<writer>","<record>"]],
 "unknown":[["<entry>",<json>]],
 "extended":[["<entry>",<json>]]}
```

- `writes`: the writer's latest own write to each field, live or not, with its
  clock reading, `first`, the number of its first own write to the field, and
  its value: `set` a register's value or a file write's file, `off` for a
  delete, a set's remove, a clear or a file write of no file, `pin` for a `pin`.
- `refs`: per field and other writer, the latest reference at or below the fork
  point of that writer its entry knew of, and, with `above`, the latest above
  it, each with its own entry.
- `killed`: per own write held, the first own delete of its entity at or after
  it.
- `takes`: per field and other writer, the latest take of a run of that writer,
  below the run's `from`.
- `dismissals`: the runs the writer dismissed, each with its entry.

A contribution holds at most one own write and one own delete per field, and per
field and other writer one take and two references, so it is bounded by the
fields its writer wrote, not by its history. Members of `state` a reader does
not know are kept; members it does not know inside a record refuse the snapshot.

Only a snapshot's writer compacts: at close, every 512 of its own entries, and
when the app asks. It confirms that a file in its directory holds its last
entry, as before an append; if none does, it writes nothing and a new writer
takes over. When a snapshot in its directory already has that head, there is
nothing to compact: it writes nothing, deletes as below only what that snapshot
lets it, which a compaction cut short left, and looks first at that snapshot
when it next confirms the entry. Otherwise it folds the entries after its
previous snapshot's head into that snapshot's state, certifies every segment of
its directory by the part of it on its chain, the snapshots of its directory on
its chain by their heads with their certificates, and every other entry it folds
by hash, syncs the snapshot and its directory, and confirms that the folder
holds it. Only then does it delete every segment in its directory that ends with
a seal marker and every line of which the new snapshot certifies, and every
snapshot whose head the new one certifies. A segment without a seal marker, such
as one left open by a crash or a copy, is never deleted, and neither is a
segment holding an entry off the chain. Just before deleting each file it
confirms that the new snapshot is still in the folder, and that the file still
has the length it read and, for a segment, ends with its seal marker; a file
that changed is left.

## Writers

The writers of an install are the directories of its local root. An instance
takes the first, in order of name, that has a readable `head.json`, no
`retired`, and whose lock it gets. It continues that writer only if what it has
read of the writer starts from its genesis entry, holds its recorded head, has
one last entry and no fork point, and a file in its directory holds that last
entry now. Otherwise it creates `retired`, releases the lock, and writes as a
new writer from its next write: a copied local root, whose history another
instance also continues, and a folder restored from an older copy, which no
longer holds the writer's last entry, each start a new writer. An instance that
finds its own history forked while it runs does the same.

A new writer is created at an instance's first write. Its first segment holds
its genesis entry; the segment, its directory and every directory above it are
synced. Only then does the writer get its directory in the local root, its lock,
its cached view and, last, `head.json`.

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
writes nothing and stops: a new writer takes over from the next write. After the
append is synced it stats the segment, writes the cached view and then
`head.json`, before the commit returns. An append that fails leaves the segment
without a seal marker and appends nothing more to it, so nothing follows a torn
line.

The entries are then durable, and the commit succeeds whatever fails after them
(see Intents). A cached view or `head.json` that cannot be written lags: the
instance writes it again at its next commit, refresh or close. A crash meanwhile
loses nothing the folder holds. Reopening reads the folder into the cached view,
and a `head.json` that names an earlier entry of the writer's chain still lets
the instance continue the writer, after the chain's last entry; one that a
failed write tore starts a new writer. Until the view is written, a crash
followed by a restore of the folder that takes the commit's entries takes them
from this install's view too.

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
reported, not used.

Per writer, from the files it reads and what it remembers, a reader derives:

- **Tips**: the writer's snapshots no other of its snapshots certifies, one per
  head.
- **Folded** entries, which it skips: one a tip names, or a line of a file whose
  first line and line at a segment certificate's position are the certificate's.
  This recognizes re-delivered files, late deletions, conflicted copies and the
  folded part of a segment that runs on past a tip's head.
- **Waiting** lines: those of a file without a seal marker that starts like a
  certified segment but is shorter, held back while the rest arrives. A sealed
  file shorter than the certificate, or one with another line at its position,
  diverged: only its first line is folded.
- **Merged** entries: one numbered one past its predecessor, which is a tip's
  head or merged, or a genesis entry when the writer has no tip.
- **Fork points**: an entry with two successors; a folded entry, not a tip's
  head, that precedes an entry no tip folds; the predecessor of a line no tip
  folds numbered at or below a tip's head, since certificates are complete; and
  where several tips part, the highest entry each of them names. Every entry and
  every tip's write numbered above the lowest fork point is a **run**, and
  neither branch is merged.
- **Gaps**: entries whose predecessor may still come, held back.
- Entries a dismissal names, which no reader merges or shows; an entry after
  one is a run.
- **Passed** entries: a remembered entry no file it reads holds, that it cannot
  merge, and whose number a tip's head has reached, is folded into that tip as
  if certified. When the tip does not certify it, it was on another branch: its
  change stays out of view, with no item, until a file holding it is read
  again, and it is then a run.

What the merge leaves out is shown as **items**: each run, gap, and after the
grace step each file's lines it cannot place, with the net change it would make
against the merged state, field by field. A tip's own write the merge leaves out
that replaced an own write of its writer no merged entry holds is a forgotten
basis: its run shows that field whatever the merged state shows, until an intent
takes a run of that writer on that field. An intent takes changes from an item
with ops that name its run in `take`, and dismisses a run with a `dismiss` op; a
gap or unplaced lines cannot be dismissed.

A writer forks when two instances write as it: a copied local root, a virtual
machine resumed twice, a restored backup of a local root. Each fork point is
reported once, and the writer retires once its own instance sees it.

Each install keeps what it remembers as a cached view in its local root:
`<genesis>/view.bin`, the whole view, and `<genesis>/view.log`, what the view
gained since, one record after another. A view holds, per writer, its tips, the
entries merged after them and shown as runs or gaps, and its fork points, with
the state of the grace step. A new writer writes `view.bin` from the view its
instance holds. After each read that changed what the view remembers, after each
append, and when the instance closes, the instance appends a record of what the
view gained to `view.log` and syncs it. It writes `view.bin` again, and then
removes `view.log`, when `view.log` has grown past half of `view.bin` and 1 MiB
more, when a record could not be appended, the last one is torn or one extends
an earlier `view.bin`, when the view's tips changed or it let go of an entry,
and when the view holds what neither file does. Each `view.bin` has a
generation, one past that of the `view.bin` it replaces, and each record of
`view.log` names the generation it extends. An instance without a writer keeps
only a change to the grace step, in the view of the writer it loaded and of each
retired writer whose view it took in.

Every open starts from the views of the install's writers joined: the view of
the writer it continues, then those of the other live writers, then those of
the retired writers that no view it has read holds already. A view also names
the retired writers of the install whose views it holds, and says what the
reader last found in each file of the writers' directories, so that a read reads
again only the files that changed. A file's record is its length, modification
time and last bytes, its stamp, and either the segment's lines, as `count`
entries of the writer's chain from `first` to `last` ending at `end`, with the
lines the view holds no entry for kept whole, or the head of the view's tip it
holds. A file the view cannot give the lines or the snapshot of is read again.

The cached view is private to the install, and only the build that wrote it
reads it. Each file is a sequence of records: the length of the record's payload
as 8 bytes little-endian, a 16-byte check of the payload, then the payload. The
check finds a record a crash tore or a disk damaged; it does not guard against
forgery, since only the install writes its local root. This build computes it in
two lanes of 64-bit multiplies over the payload. The payload's first byte names
its format, 4; then come the generation, and the view, or what it gained, in
this build's binary encoding: integers as LEB128, identifiers as their 16 bytes
big-endian, text as its length and its UTF-8, lists as their length and their
members, with each entry as its hash, its predecessor, its number, its clock
reading and its decoded kind. An entry keeps the JSON of its line only when that
is not the JSON this build writes for its kind, so every line is given back byte
for byte. Neither a line nor its hash is read or checked again: the record's
check stands for them. A record whose length, check or format byte does not hold
ends the readable part of `view.log`. A reader replays the records in order onto
`view.bin`, up to the first that is torn, skipping each that extends another
generation, as a crash between writing `view.bin` and removing `view.log` leaves
them; replaying a record twice changes nothing. A view that cannot be read this
way is discarded, and the folder read from scratch.

### The grace step

A reader keeps showing what it remembers that no file it sees holds: a tip
whose file is gone, or an entry in a file sync has not brought, or one a restore
of the folder from an older copy took. Once that has lasted 24 hours and at
least one scan of every file has finished since, it takes the grace step: it
lets go of each such tip and entry from which nothing it sees descends, a line
by its chain of predecessors or a snapshot by its certificates, forgets the fork
points the files no longer show, and derives again. From then on an unsealed
file shorter than a certificate no longer waits, and a reference to a number
above the highest the files show of its writer, or to a number where the files
show only another entry, kills only the write with its hash. Facts only a
restore took then disappear, without a prompt.

## Binding

Which library file is an entity's is derived, never logged as such. A binding is
a function of the merged file registers, the library files a reader sees and
their identities:

1. A scan lists every library file with its length and modification time. It
   reads an identity only when a file's length is that of some file fact, and no
   fact, earlier scan or `identities.json` gives the identity for that path,
   length and time. The install keeps in `identities.json` the identities its
   scans read while their files keep their length and time. After a commit, only
   the paths its file effects moved files from and to are scanned again, and of
   the other files only those whose length a file fact gained have their
   identities read. A refresh scans again only the paths named by the file facts
   the entries it read changed, before and after, and the paths those entities
   were bound to, with the same identity reads. A rescan, which the app asks for
   once a library opens and when something outside may have changed the folder,
   scans every file. Opening scans none: until a scan of every file succeeds,
   every library path is unknown. A scan that fails once a commit's entries are
   durable does not fail the commit. Until a scan of every file succeeds, the
   paths it would have scanned and what is under them are unknown: no file there
   is bound, reported or a candidate for a move. The entities bound there before
   the commit, and those whose file facts name such a path, are bound to no file
   and shown at their logged path as unscanned. Nothing about them is pinned, and
   each refresh scans every file until a scan of every file succeeds.
   A scan of every file lists one directory at a time and keeps what it listed.
   One that stops before every directory is listed binds nothing; the next goes
   on where it stopped, listing again each directory holding a path a commit or
   refresh scanned meanwhile, and each directory at or under such a path. A
   change outside the app to a directory already listed shows at the next scan
   of every file, or of its path.
2. An entity whose file fact names a path a file is at is bound to it: in sync
   when the file holds the fact's identity (or, without one, its length and
   time), else changed outside. Paths compare under the volume's rules for case
   and Unicode normalization. When several entities name one file, the one whose
   identity it holds gets it.
3. An entity whose path holds nothing is bound to the one unbound file holding
   its identity: a move. With no such file it is missing; with several, or when
   one file could be several entities', nothing is bound and the candidates are
   reported.
4. A file holding a bound entity's identity while that entity's own path still
   holds it is a copy: a new file with no entity until an intent says something
   about it.

Every commit pins the moves this writer holds that its facts do not say yet: each
file in sync at another path, as a `pin` op in a `bind` entry. A new
modification time alone is not logged; it only spares a scan reading an
identity. Conflicted file registers, changed files and missing ones are left for
the user. A scan writes nothing in the folder.

## Intents

An intent is one user action: fact changes and file effects committed together.
Opening, reading, refreshing and scanning write nothing in the folder; only
committing an intent (an undo, a redo or a settlement included), emptying the
trash, tidying, compaction and closing do.

A commit:

1. Turns the fact changes into ops against the merged state: a register write
   replaces every live write of the register, an add or remove every live write
   of its member, a delete observes every live field write of its entity, and
   the intent names the fork points it knows of the writers its ops name. Each
   change taken from an item names the item's run in `take`. A key the app did
   not declare, or an entity that is not shown, refuses the intent.
2. Turns the file changes into the steps below and their preconditions.
3. If the instance has no writer yet, checks the preconditions without writing
   and only then creates its writer, so a refused first intent leaves nothing.
4. Settles this writer's own unfinished effects (see Recovery).
5. Carries out the file effects under a pending record, then appends the intent
   with a `file` op for each entity's file as the folder shows it afterwards,
   and after it the `bind` entries that pin the moves found.

Building an intent writes nothing, but draws the id of each entity it creates,
so one intent can create entities that name each other.

A commit whose file effects stop partway is logged with what they did, and
fails, saying where they stopped. When settling one of this writer's unfinished
effects at step 4 stops partway, that settlement is logged and the commit fails
before its own intent is tried.

Otherwise a commit whose entries are durable succeeds, whatever fails after
them, and says which parts of the instance's own state lag it: the cached view
or `head.json` not written, which the next commit, refresh or close writes
again; or the scan after it failed, which the next refresh repeats. Settling one
of this writer's own effects and settling another writer's effect commit in the
same way.

An intent may also adopt a library file no entity is bound to: a precondition on
its identity, no step, and a `pin` op giving the entity the file as it is.

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
by one `rename` per file the last scan saw, then `remove_dir` deepest first.

Where the folder cannot rename files, each move copies instead, and directories
move file by file. The record says so with `"moves":"copy"`, and whoever carries
out its steps, the writer resuming them included, moves the same way.

A writer carries out an intent's steps in this order:

1. Create each new file in `tmp/` and fill it, then sync it and `tmp/`.
   The app's bytes are written a chunk at a time by the driver, never held by
   the core; they may copy ranges of the file being rewritten. A copy out of
   another writer's directory is made a chunk at a time. A file whose filling
   fails is removed.
2. Check every precondition: a path holds nothing, or a file whose identity is
   the one the app expects; a directory satisfies neither. Check that every
   trash item a step restores is there. On failure, remove the staged files and
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
   only then removes the source and syncs the source's directory. A copy that
   fails removes what it wrote. Each step runs alone.
5. Append the intent's entry.
6. Keep the pending record, now spent: its entry is logged, so the record is
   not open, and no reader that has the entry reports it. No commit removes it.
   The writer removes its spent records, each after its markers, when the app
   tidies, as an app does when idle, and when it closes. A record whose removal
   fails stays spent for the next tidy or close.

Where the folder's renames refuse an existing destination themselves, as
`renameat2` with `RENAME_NOREPLACE`, `renamex_np` with `RENAME_EXCL` and
`MoveFileExW` without `MOVEFILE_REPLACE_EXISTING` do on volumes that support
them, a move never replaces anything. Elsewhere a rename may replace, and the
check before it is the only guard: a file another program makes at the
destination between the check and the rename is replaced, and its bytes leave
the folder. A move that copies creates its destination, which replaces nothing
where the backend's creates refuse an existing file in the same step; a
browser's do not (see Browsers). Opening says which kind of folder it is.

If a step fails, the rest are not tried: staged files not placed are moved to
`trash/<their nonce>`, replacing what a move cut short left there, the entry
records the effects that were made, and the intent reports that it stopped
partway. Each library path holds its old bytes, nothing, or its new bytes, and
while it holds nothing a pending record names it. Where moves copy, a library
path may also hold a copy of its new bytes cut short while a pending record
names it: no longer than them, holding their start or, where a crash zeroed or
tore bytes the volume had not yet synced, anything else.

Removing a record first removes its markers and syncs `pending/`.

A pending record is a JSON object:

```json
{"writer":"<w>","entry":<entry>,"label":"Save","steps":[<step>,…],"files":[{"entity":"<e>","path":"a/b.syx","done_after":2}]}
```

`entry` is the intent's entry as planned, without its `file` ops and
`displaced`, whose `prev` is the writer's head when the record was written.
`"moves":"copy"`, omitted otherwise, says the steps move files by copying.
`files` says where each entity's file is once the first `done_after` steps are
done; a `path` of `null` is no file. `"pin":true` marks a file the intent adopts
as it is, logged as a `pin` op rather than a `file` op. A reader reads at most
16 MiB of a record.

### Recovery

A record is open while its writer's log holds the entry named by its entry's
`prev` and no entry after it. A record that is not open is spent, waiting only
for its writer to remove it.

A record is ignored when it does not decode, names another writer, names a path
that is not a library path, or its `prev` is not in its writer's log. Opening
reports the records it ignores.

Opening finds this writer's open records whose `prev` is its head, and says how
settling each will end. The writer settles them before it checks its next
intent, undo, redo or settlement against the view: it finds how far the steps
got by checking them from the last, and the first that shows done ends the done
prefix. `to_trash` shows done when its trash item exists; any other move when
its destination exists and its source does not, or both hold the same bytes. A
`place` whose staged file is in the trash under its own nonce is not done. When
the last done step's source still holds the same bytes as its destination, the
source is removed. The remaining steps are carried out, the planned entry is
appended with what the steps did, and the record is spent. Then every file in
its `tmp/` that no open record places is removed, and so is every
`pending/<nonce>.json` opening ignored that is a record created in place and cut
short before its bytes landed, before any of its steps ran: one that is empty
or, in a folder that syncs, one of at most 16 MiB that holds a zero byte or is
the start of a JSON value. Any other record that does not decode stays.

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
as an intent of its own: to finish, it carries out the remaining steps, skipping
a `to_trash` whose path holds nothing, and logs the facts the record planned; to
roll back, it reverses the done ones; to dismiss, it changes no file. It finds
how far the steps got by checking them from the last, whichever way they move,
since its own settling may have carried them further or reversed some; a
`to_trash` of a record whose moves copy shows done as any other move does. A
copy cut short, as above, at the destination of the record's last marked step
makes that step the one in progress and goes to the settler's trash first, as
does a library path the last done step left beside its destination. It
copies files out of the other writer's `tmp/` and `trash/` rather than moving
them, and moves a library file it displaces into its own trash. Once its steps
are done, it appends a `settle` entry naming the writer and record, after which
no reader reports the record. If its steps stop partway, the settlement fails
and its intent is logged with what they did, without the planned facts or a
`settle` entry. The record stays open, and settling it again finds from the
folder what remains. Only the record's writer removes it.

A record written where directories rename may move a tree by one `rename`. A
reader whose folder cannot rename directories refuses to finish that step or roll
it back, and changes nothing; it can still dismiss the record, and a folder that
renames directories can settle it.

## Undo and redo

Each instance undoes only the intents it committed since it opened, most recent
first, by committing a compensating intent. A redo reverses the latest undo
while the instance has committed nothing else since. Undo history is not logged:
it ends when the instance closes. Each intent is kept with what it replaced, as
the merged state showed it when it committed.

- A register write is reversed by writing back the latest value it replaced,
  another writer's included; a clear where there was none.
- An add is reversed by a remove, a remove by an add.
- A create is reversed by a delete, and a delete by a create replacing it, with
  the register values, members and file the delete ended.
- A `file` op is reversed by putting back what it changed: the displaced bytes
  from the trash, under a precondition on what the path holds now; the old name;
  or the trash, for a file the intent added.
- `pin` ops are left alone, so undoing an adoption leaves the file.

An undo or redo is refused, naming the writer and entry, where another writer
changed the same thing since: a register no longer holds only what the intent
left, another writer added or removed the same value, a delete of the entity is
live, or another writer wrote to an entity being uncreated. It is refused when
the trash no longer holds the bytes it needs.

## Trash

`trash/<nonce>` holds bytes an intent of the writer displaced, or new bytes an
intent staged and could not place. An item is listed with the entry whose
`displaced` names it. Only its writer removes an item, and only by emptying:
first every item displaced longer ago than the policy's age, by default 30 days,
then the oldest items until the rest fit the policy's size, by default 1 GiB.
An item no entry names is never emptied. Emptying removes the items, then syncs
the trash once; an item whose removal a crash undid is removed by the next
emptying.

Emptying a trash is the only way bytes leave the folder.

## Drafts

`<genesis>/drafts/<entity>.json` is the JSON line `{"base":"<identity>"}`, an LF,
then the unsaved bytes. It is kept only in the local root, so losing the local
root loses drafts and nothing else. Opening restores a draft only while the
entity's file holds `base`; otherwise it reports what the file holds now. An
instance that has never written has no directory in the local root and keeps no
drafts.

## What stays

The folder keeps, for the life of a writer: one snapshot, holding at most one own
write per field it wrote and per other writer two references and one take, and a
certificate per segment it ever wrote; the segments written since; one segment
per process that crashed or was copied; trash items
until the writer empties them; spent pending records until the writer tidies or
closes; and the pending records of a writer that will never run again, settled
or not. The local root keeps one directory per writer
the install has used.

## Backends

A backend declares, for each root, what it can do, and the core plans from that
alone:

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
and a completed request is as durable as the backend makes it.

The native backend tells a volume by the type of file system it reports, read at
the root or, before the root exists, at the nearest directory above it. It
declares `rename_file` and `rename_dir` except where the file system keeps no
journal of its names, so that a crash can keep a rename's removal of the old
name without the new one, and where it cannot tell: FAT, exFAT, NTFS, HFS, HFS+
and UDF on Linux; FAT, exFAT, NTFS and UDF on Apple systems; FAT, exFAT and UDF
on Windows; every FUSE file system, which hides what it stores to; and a volume
whose type cannot be read. There every move copies. The type is all it reads:
ext2, and ext4 made without a journal, report ext4's type and are taken to
rename atomically, as is any file system not named here. It declares
`no_replace` where renames also refuse an existing destination (see File
effects). On Windows it declares no `fsync`, so a copy cut short is told only by
holding the start of its source's bytes, and a record cut short only by being
empty (see Recovery).

### Browsers

In a browser, the library runs on the page and a dedicated worker performs its
requests, one at a time, in the order they were sent. The folder is a directory
of the origin private file system or a folder the user picked; the local root is
always a directory of the origin private file system.

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
  disk is inferred from its cost, not confirmed. A directory's names live in
  the browser's own database, which toshokan cannot sync, so a sync of a
  directory returns at once, and a name made or removed is only as durable as
  that database keeps it; that it survives the machine stopping is not
  confirmed. Chromium cannot rename a directory there, so no browser declares
  `rename_dir`.
- **Picked folder.** Only Chromium-based browsers can open one. Files are read
  through `getFile()` and written through writable streams, which write a copy
  and put it in place at `close()`: that is the moment a write lands, and
  nothing is synced beyond it. An append copies the whole file, at about 1.5 ms
  per MiB, which the 1 MiB bound on segments keeps small. The writes that fill
  one file share one stream, kept open while other files are read. A file
  created with bytes is named first and filled at its stream's close, so a tab
  stopped between leaves it empty. Before
  putting a file in place, Chromium checks it with Safe Browsing, in full unless
  its type is one Chromium samples, such as `.txt` and `.json`, so toshokan
  names its own files with those. A `.txt` close was measured at about 1.6 ms in
  Chrome; that `.json` is sampled too, and that about 1 close in 100 of a
  sampled type still pays the full check, is from Chromium's published file-type
  policy, not measured. A saved library file is staged under its own
  extension, so it pays what the browser charges for its type: the full check,
  about 45 ms in Chrome and 0.1 to 1.1 s in Brave, for a type Chromium does not
  sample. A trash item has no extension, so where moves copy, each file moved
  into the trash pays the full check. Brave cannot rename a file outside the
  private file system. Renaming there was measured only in Chrome and Brave, so
  `Folder::picked` declares `rename_file` only where `navigator.userAgentData`
  names Google Chrome and `navigator.brave` is absent; elsewhere each move
  copies, and each copy pays the check again. A rename the browser refuses
  although the folder declares it fails with `Unsupported`.
- **Directories.** A library tells the worker its root as it opens. The worker
  keeps the handles of the directories it finds under that root and in the
  local root, where no writer removes or renames a directory, and finds each
  other directory from the folder's top for every request. Several reads of one
  such directory find its files by one listing of it. Firefox was measured to
  take about 3.5 ms for each step of a path, and 6 ms for each file's handle,
  once the origin holds 100k files.
- **Renames** replace a file at the destination in every browser, so none
  declares `no_replace`. **Creates** check that nothing is there and then make
  the file, which opens one already there, so a file another program makes
  between the two is replaced by a copy's bytes, or removed when the copy finds
  it too long, as a rename replaces one.
- **Swap files.** While a writable stream is open, Chromium keeps its bytes in
  `<name>.crswap` beside the file, and a tab stopped meanwhile can leave that
  file behind. That it shows while the stream is open was observed; that a
  stopped tab leaves it is inferred, not confirmed. toshokan reads, scans and
  reports no file whose name ends in `.crswap`, wherever it is, and refuses
  such a name as a library path.
- **Times.** Chromium and Firefox report a file's last change in milliseconds,
  WebKit in whole seconds: two changes that keep a file's length within that
  time look alike.
- **The page's thread.** An operation pauses between slices of work that makes
  no request, such as parsing, placing and folding the logs it read, decoding
  and encoding the cached view, gathering, identifying and binding the library's
  files, and indexing what is shown. Once the core has held the page's thread for 6 ms since
  a reply or the last pause, the driver gives it back at the next pause by a
  message the page posts to itself, so the tasks waiting for the thread run
  first and the page can draw. Where an operation pauses depends only on its
  work, never on time.
- **Locks** are Web Locks named `toshokan:/<local root path>/<name>`, held by
  the worker and released when it ends.
- **Hints.** A tab that commits announces its newest entry on the
  `BroadcastChannel` `toshokan:<library>`, and the library's other tabs refresh.
  Where the browser has a `FileSystemObserver`, which only Chromium has, a page
  watches its folder and rescans the paths the observer reports changed, or
  every file when it cannot say which. A page rescans every file once a library
  opens; one whose folder another program may write also rescans every file when
  it is shown or focused, and periodically while it is visible.

