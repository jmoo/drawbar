# toshokan format

This document specifies the bytes toshokan writes and reads, precisely enough
for a second implementation to share a library with this one.
[protocol.md](protocol.md) specifies how writers and readers behave. A library
lives in a folder that a sync client may deliver late, out of order, partly
restored or with conflicted copies, and the rules here are written for it. The
format may change before 1.0.

All text is UTF-8. All hexadecimal is lowercase. JSON is written without
insignificant whitespace.

## Layout

The app chooses a library folder and the name of a root directory inside it,
such as `.drawbar`, or a root inside a directory of the app's own, such as
`.drawbar/library` inside `.drawbar`. In the folder, toshokan writes only under
that root, and in the library's files only as an intent's file effects ask.
Every path in this table is relative to the root.

| Path                                  | Contents                                |
| ------------------------------------- | --------------------------------------- |
| `writers.2/<w>/<segment>.txt`         | A segment of writer `w`'s log           |
| `writers.2/<w>/snapshot-<nonce>.json` | A snapshot of writer `w`'s log          |
| `writers.2/<w>/pending/<nonce>.json`  | The record of an unfinished file effect |
| `writers.2/<w>/pending/<nonce>.<i>`   | Empty: step `i` of `<nonce>` has begun  |
| `writers.2/<w>/trash/<nonce>`         | Bytes an intent of `w` displaced        |
| `writers.2/<w>/tmp/<nonce>.<ext>`     | A file `w` is staging                   |

Only writer `w` creates, appends to, renames or removes anything under
`writers.2/<w>/`. Others only make and sync the directories `writers.2/`, the
root and the app's directory around it. There is no file at the level of the library.

A staged file is named by its nonce and the extension of the file it becomes:
what follows the last `.` of that file's name, if anything does, and if the name
stays within 255 bytes; otherwise by its nonce alone. A trash item is named by
its nonce alone, so a step names it without knowing where its bytes came from.

Names of segments and snapshots are advisory. A reader reads every file directly
in `writers.2/<w>/`, whatever its name, by its contents, so a sync client's
conflicted copy is read like any other file. A browser's swap file is the
exception (see [Browsers](protocol.md#browsers)).

A **library path** is a path in the folder outside the root and outside the
app's directory around it, other than the folder itself and a browser's swap
file, and, where the app's layout hides them, other than a path with a name
that starts with a dot. Only library paths are the user's files.

Each install keeps, per library, a local root of its own that is never synced:

| Path                  | Contents                                                     |
| --------------------- | ------------------------------------------------------------ |
| `<genesis>/head.json` | The last entry this writer wrote, and whether it is retired  |
| `identities.json`     | Identities the install's scans read                          |

toshokan reads and writes nothing else there, so the app may keep files of its
own beside these, under names that are not a genesis hash.

`<genesis>` is the hash of the writer's genesis entry. The directories of the
local root are the install's pool of writers (see
[Writers](protocol.md#writers)).

`head.json` is `{"writer":"<writer id>","head":"<entry hash>"}`, with
`"retired":true` last once the writer is never written again, and
`identities.json` is
`[["<library path>",<length>,<modification time or null>,"<identity>"],…]`.
A writer's lock is taken on its `head.json`; a backend that holds locks as file
locks keeps the file readable and writable while it is held. So `head.json` is
never replaced, which would make a new file another process could lock: it is
written over in place and synced. Every record of one writer has the same
length but the retired one, which is longer and written last, so no write leaves
the end of an earlier record behind. A crash can tear that write. What it leaves
is the old or the new record whole, bytes that do not read, which leave the
writer out of the pool, or a record naming an entry no log holds, which retires
the writer (see [Writers](protocol.md#writers)). A file in the local root that
is replaced, such as `identities.json`, is first written beside it as
`<name>.next` and synced; then `<name>` is removed and `<name>.next` renamed to
it. A reader takes `<name>`, or `<name>.next` when `<name>` is missing.

## Compatibility

`writers.2` is version 2 of this format. Builds of different versions read and
write one library at once, as computers that sync a folder do, and an open page
keeps running the build it loaded. Within version 2:

1. **Frozen.** The line form and the seal marker; an entry's `prev`, `n`, `at`
   and `kind`; ids as 32 hex digits; every array form (clock readings, `Ref`,
   `Own`, `Forked`, certificates, `settled` pairs, `unknown` pairs, the rows of
   `identities.json`); the names of every file and directory in Layout; the lock
   names; and the limits on a line, a record and a file. Frozen too: the members
   of each record of a snapshot's `state` and of an observed write, since a reader
   refuses what it does not know in them, and of `head.json`, which is written over
   in place, so a reader drops what it does not know there.
2. **Additive.** Anywhere else, a new object member may appear. It is optional,
   changes nothing about the members beside it, and a reader that does not know
   it keeps it where [Merging](protocol.md#merging) and [Snapshots](#snapshots)
   keep what they do not know. Data a
   newer build adds to a snapshot's records goes in a new member of `state`.
3. **New meaning is a new name.** A change of meaning, a required member, or a
   new way to close a pending record is a new `kind`, `op`, `step` or `moves`
   value; `outcome` keeps its three values, and only an `intent` naming the record
   or a `settle` closes one. A reader keeps such an entry unknown, opens
   read-only when its own writer's history holds an unknown kind or op, never
   finishes a record it cannot decode, and never deletes a file it cannot read.
4. **App values are the app's.** A value a reader cannot decode is shown
   unreadable, kept and merged as any other.
5. **Anything else is a new layout.** A library whose root holds `writers.<k>`
   for a `k` past 2 opens read-only.
6. **Fixtures hold it.** Every state under `tests/fixtures` reads to its oracle in
   every later build, and every fixture of a newer version behaves as its README
   says.

There are no migrations before 1.0. A folder of an earlier format, under
`writers/`, opens as an empty library whose files are all unbound; nothing
reads its directories, which are left for older builds. A local
root's `view.bin` and `view.log`, which older builds kept, are ignored, as are
old `head.json` files.

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

The identity function is the app's: a fingerprint of a file's contents from the
ranges of its bytes the app names for the file's path and length. It may read
only a few ranges of a file only where the file's format holds a checksum that
covers every other byte of the file, and must read every byte of any other: an
identity of part of a file would take a file changed elsewhere for the one it
was. toshokan reads those ranges and feeds them to the app's digest, and never
computes an identity itself.

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

A line longer than 16 MiB, a line whose hash does not match, a final line without
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
not know, at any depth but an observed write's (see
[Compatibility](#compatibility)), and keeps the line and the whole entry (see
[Merging](protocol.md#merging)), so they survive. An entry of another kind,
without `kind`, or whose members do not decode, is kept and merged as unknown; so is a line whose JSON has no readable
`n` or `at`, which still links the chain but is never merged.

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
| `record`    | The nonce of the pending record whose effects it logs, which it closes; omitted if none |

Each op is an object whose `op` member names it:

| `op`      | Members                                          | Meaning                                      |
| --------- | ------------------------------------------------ | -------------------------------------------- |
| `create`  | `entity`, `replaces`, `take`?                    | The entity exists                            |
| `delete`  | `entity`, `replaces`, `observed`, `take`?        | The entity is deleted                        |
| `write`   | `entity`, `key`, `value`, `replaces`, `take`?    | A register holds `value`; cleared without it |
| `add`     | `entity`, `key`, `value`, `replaces`, `take`?    | A set gains `value`                          |
| `remove`  | `entity`, `key`, `value`, `tags`, `take`?        | A set loses `value`                          |
| `file`    | `entity`, `file`, `replaces`, `take`?            | A file effect left the entity's file here; none without `file` |
| `pin`     | `entity`, `file`, `replaces`, `take`?            | A reader bound the entity's file here        |
| `dismiss` | `runs`                                           | The user dismissed these runs                |
| `path`    | `from`, `file`, both optional                    | A file effect moved a file no entity's from `from` to `file` |
| `tree`    | `from`, `to`                                     | A file effect moved the tree at `from` to `to`, which it made |
| `dir`     | `path`, `made`                                   | A file effect made the directory `path` (`true`) or removed it (`false`) |

`replaces` and `tags` are arrays of references to the writes of the same field
the writer saw live: of a register, of a set's member, or of the file register.
`observed` names the live field writes of the entity a delete saw, each as
`{"key","at"}` for a register, `{"key","value","at"}` for a set's member or
`{"file":true,"at"}` for the file register, with `at` the reference. A `value`
is the app's JSON, kept byte for byte; `"value":null` writes `null`. A `file` is
`{"path","identity","len"}` with an optional `modified`, the backend's
modification time, compared only for equality. Each member of `displaced` is
`{"item","from","identity","len"}`: the trash item's nonce, the library path the
bytes left, their identity and length. An op of another name, missing a member,
or with an observed write holding a member the reader does not know, is kept and
unknown.

A run, which `take` and `runs` name, is `{"writer","from","first"?,"last","tip"?}`:
the writer's entries numbered from `from` to `last` on `last`'s chain, the first
by its hash; a run a snapshot holds is named by that snapshot's head as `tip`,
and its first entry by number alone (see [Runs](protocol.md#runs)).

For example:

```json
{"prev":"…02","n":3,"at":[3,0],"kind":"intent","label":"Tag","ops":[{"op":"add","entity":"…0e","key":"tags","value":"x","replaces":[]},{"op":"write","entity":"…0e","key":"origin","replaces":[["…0b",4,"…01"]]}]}
```

An intent is one line, file ops and `displaced` items included. Before it writes
anything, a commit refuses an intent whose line would be longer than a line can
be with the facts a settlement adds, every file op and `displaced` item its
effects may add whether they finish or stop, and a fork point for every writer it
names, each as long as it can be, or whose pending record would be longer than a
reader reads (`Invalid::TooLong`). A file op replaces the writes the intent's
writer knew of when it planned it, which finishing another writer's record
keeps, so no entry that logs the intent, at commit or when a record settles, is
longer. A commit appends its entries in one order: the
intent, the `settle` entry when it closes another writer's record, then its
`bind` entries, each holding at most 1 MiB of ops.

A **settle** entry records that this writer settled another writer's unfinished
effect with the user's consent:

| Member    | Value                                          |
| --------- | ---------------------------------------------- |
| `writer`  | The writer whose pending record it settles     |
| `record`  | The record's nonce                             |
| `outcome` | `"finished"`, `"rolled-back"` or `"dismissed"` |

A **bind** entry pins moves this writer's reader found (see
[Binding](protocol.md#binding)), after the
intent it committed with:

| Member | Value         |
| ------ | ------------- |
| `ops`  | Its `pin` ops |

Its ops are merged as an intent's are, and it is never undone. A commit spreads
its pins over as many `bind` entries as keep each line within the limit, so any
number of moves can be pinned.

Nothing derived is logged: views, bindings a reader has not pinned, and the
merged state exist only in readers and in snapshots.

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

`certified` is `{"segments"?,"hashes"?}`. Each member of `segments` is
`[[<n>,"<hash>"],[<n>,"<hash>"]]`: a segment the chain was written to, by its
first line, and the last of its lines the snapshot folds. `hashes` names every
other folded entry and the last entry of each dismissal of the writer's own runs
on its chain. The snapshot's head is among the entries these name, so the
certificates of the snapshots it folds name their heads. A reader ignores other
members of `certified`, such as a `heads` list, whose entries the rest already
name.
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
  point of that writer its writer knows of, and, with `above`, the latest above
  it, each with its own entry. A reference made before its writer knew of a
  fork, to a write above it, moves above it at the writer's next reference on
  that field to that writer.
- `killed`: per own write held, the first own delete of its entity at or after
  it.
- `takes`: per field and other writer, the latest take of a run of that writer,
  below the run's `from`.
- `dismissals`: the runs the writer dismissed, each with its entry.
- `settled`: each pending record the writer closed, by its writer and nonce: its
  own, by the intent naming it, and another writer's, by a settlement.

A contribution holds at most one own write and one own delete per field, and per
field and other writer one take and two references, so these are bounded by the
fields its writer wrote, not by its history; `settled` gains a pair per record
closed (see [Bounded retention](protocol.md#bounded-retention)). Members of
`state` a reader does not know are kept; members it does not know inside a
record refuse the snapshot.
