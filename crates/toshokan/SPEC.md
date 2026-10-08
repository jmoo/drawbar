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
root must rename files.

## Layout

The app chooses a library folder and the name of a root directory inside it,
such as `.drawbar`. In the folder, toshokan writes only under that root, and in
the library's files only as an intent's file effects ask. Every path in this
table is relative to the root.

| Path                                | Contents                                 |
| ----------------------------------- | ---------------------------------------- |
| `writers/<w>/<segment>.txt`         | A segment of writer `w`'s log            |
| `writers/<w>/snapshot-<nonce>.json` | A snapshot of writer `w`'s log           |
| `writers/<w>/pending/<nonce>.json`  | The record of an unfinished file effect  |
| `writers/<w>/pending/<nonce>.<i>`   | Empty: step `i` of `<nonce>` has begun   |
| `writers/<w>/trash/<nonce>`         | Bytes an intent of `w` displaced         |
| `writers/<w>/tmp/<nonce>.<ext>`     | A file `w` is staging                    |

Only writer `w` creates, appends to, renames or removes anything under
`writers/<w>/`. Others only make and sync the directories `writers/` and the
root. There is no file at the level of the library.

A staged file is named by its nonce and the extension of the file it becomes:
what follows the last `.` of that file's name, if anything does, and if the name
stays within 255 bytes; otherwise by its nonce alone. A trash item is named by
its nonce alone, so a step names it without knowing where its bytes came from.

Names of segments and snapshots are advisory. A reader reads every file directly
in `writers/<w>/`, whatever its name, by its contents, so a sync client's
conflicted copy is read like any other file. A browser's swap file is the
exception (see Browsers).

A **library path** is a path in the folder outside the root, other than the
folder itself and a browser's swap file. Only library paths are the user's
files.

Each install keeps, per library, a local root of its own that is never synced:

| Path                             | Contents                                     |
| -------------------------------- | -------------------------------------------- |
| `<genesis>/head.json`            | The last entry this writer wrote             |
| `<genesis>/view.json`            | The cached view                              |
| `<genesis>/view.log`             | What the cached view gained since            |
| `<genesis>/drafts/<entity>.json` | An unsaved edit                              |
| `<genesis>/lock`                 | Held while an instance writes as this writer |
| `<genesis>/retired`              | Empty; the writer is never written again     |
| `let-go.json`                    | Entries the install let go, by writer        |
| `identities.json`                | Identities the install's scans read          |

`<genesis>` is the hash of the writer's genesis entry. The directory is created
only after that entry is durable in the folder, so the directories of the local
root are the install's pool of writers.

`head.json` is `{"writer":"<writer id>","head":"<entry hash>"}`,
`let-go.json` is `{"<writer id>":["<entry hash>",…]}`, and `identities.json` is
`[["<library path>",<length>,<modification time or null>,"<identity>"],…]`.
A file in the local root that is replaced, such as `head.json`, `view.json` or a
draft, is first written beside it as `<name>.next` and synced; then `<name>` is
removed and `<name>.next` renamed to it. A reader takes `<name>`, or
`<name>.next` when `<name>` is missing.

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
| `at`   | Its clock reading                               |
| `kind` | `"genesis"`, `"intent"`, `"settle"` or `"bind"` |

followed by the members of its kind. A reader ignores members it does not know,
at any depth, and keeps the line and the whole entry (see Merging), so they
survive. An entry of another kind, without `kind`, or whose members do not
decode, is kept and merged as unknown; so is a line whose JSON has no readable
`at`, because it still links the chain.

A **genesis** entry starts a writer's chain, with `prev` all zeros:

| Member   | Value                              |
| -------- | ---------------------------------- |
| `writer` | The writer's id                    |
| `label`  | The name other writers show for it |

A genesis entry after another entry is unknown.

An **intent** entry logs one committed intent, the unit of commit, undo and
attribution:

| Member      | Value                                                   |
| ----------- | ------------------------------------------------------- |
| `label`     | What the user did                                       |
| `ops`       | The fact changes, in order                              |
| `displaced` | Bytes it moved into the writer's trash; omitted if none |
| `reverses`  | The entry it undoes or redoes; omitted if none          |

Each op is an object whose `op` member names it:

| `op`     | Members                                 | Meaning                                      |
| -------- | --------------------------------------- | -------------------------------------------- |
| `create` | `entity`, `replaces`                    | The entity exists                            |
| `delete` | `entity`, `replaces`, `observed`        | The entity is deleted                        |
| `write`  | `entity`, `key`, `value`, `replaces`    | A register holds `value`; cleared without it |
| `add`    | `entity`, `key`, `value`                | A set gains `value`                          |
| `remove` | `entity`, `key`, `value`, `tags`        | A set loses the adds of `value` `tags` names |
| `file`   | `entity`, `file`, `replaces`            | A file effect left the entity's file here; none without `file` |
| `pin`    | `entity`, `file`, `replaces`            | A reader bound the entity's file here        |

`replaces`, `observed` and `tags` are arrays of entry hashes. A `value` is the
app's JSON, kept byte for byte; `"value":null` writes `null`. A `file` is
`{"path","identity","len"}` with an optional `modified`, the backend's
modification time, compared only for equality. Each member of `displaced` is
`{"item","from","identity","len"}`: the trash item's nonce, the library path the
bytes left, their identity and length. An op of another name, or missing a
member, is kept and unknown.

For example:

```json
{"prev":"…02","at":[3,0],"kind":"intent","label":"Tag","ops":[{"op":"add","entity":"…0e","key":"tags","value":"x"},{"op":"write","entity":"…0e","key":"origin","replaces":["…01"]}]}
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

The merged state of a set of entries is a join: folding an entry, and joining
two states, commute, associate and are idempotent. Readers holding the same
entries compute the same state whatever was compacted and in whatever order the
files arrived. Every write is keyed by the entry that made it.

Keys are declared by the app as registers or sets, each under a name. A key's
values are the app's JSON.

- **Registers** are multi-value. A register keeps every write it has seen and a
  grow-only set of replaced entry hashes, the union of every write's
  `replaces`. The writes no write replaces survive. Surviving writes of
  different values are a conflict; equal values are one value. A reader shows
  the latest by clock reading, then by writer id, and lists the rest. A write
  that replaces every survivor resolves the conflict. A surviving clear holds no
  value, so a clear concurrent with a value leaves the value. Because the
  replaced set travels with the state, a snapshot never brings back a write that
  another log replaced.
- **Sets** are observed-remove. An add's tag is the pair of its value and its
  entry. A remove takes away the tags it names, so an add the remover had not
  seen survives. The state keeps each removed tag with its earliest remove.
- **Existence** is a register whose writes are `create` and `delete`. A delete
  records the field writes it observed; writes in the delete's own entry count
  as observed. An entity is shown while a create survives, or while a delete
  survives together with a live field write it did not observe: a surviving
  register value, a set add, or a file. That is a deletion conflict; a `create`
  replacing the delete settles it in favor of the entity.
- **Files** are a register per entity whose writes are `file` and `pin` ops.
- **Trash items** are kept per writer and item from the `displaced` lists, and
  **settled records** from the `settle` entries, so compaction keeps them.
- **Unknown** entries and ops are kept with the entry that holds them. An entry
  of a known kind is merged and also kept whole when its JSON holds a member,
  at any depth, that the reader leaves out of what it decoded. Member order and
  spacing do not count.

A value that does not decode as the key's declared type is shown as unreadable,
and kept and merged like any other.

## Snapshots

`snapshot-<nonce>.json` is one JSON object, without a final newline:

| Member   | Value                                                         |
| -------- | ------------------------------------------------------------- |
| `writer` | The writer's id                                               |
| `label`  | The writer's label                                            |
| `at`     | The clock reading of the last folded entry                    |
| `folded` | The hash of every entry it folds, from the genesis entry on, each the successor of the one before it |
| `state`  | The merged state of those entries                             |

Members a reader does not know are kept, and a compaction carries those of the
snapshots it folds into the new one, keeping the greater JSON text under each
name. A reader refuses a `folded` list that is empty or repeats a hash. Because
the list is whole, a reader can place an entry after any folded entry and see a
fork from any point.

`state` is an object:

```text
{"entities":{"<entity>":{
   "existence":{"writes":[{"entry","by","at","deleted"?}],"replaced":[…]},
   "registers":{"<key>":{"writes":[{"entry","by","at","value"?}],"replaced":[…]}},
   "sets":{"<key>":{"adds":[{"value","entry","by","at"}],
                    "removed":[{"value","entry","by","at","remove"}]}},
   "file":{"writes":[{"entry","by","at","file"?}],"replaced":[…]}}},
 "trash":[{"writer","item","entry","at","from","identity","len"}],
 "settled":[["<writer>","<record>"]],
 "unknown":[["<entry>",<json>]],
 "extended":[["<entry>",<json>]]}
```

`by` is the writing writer's id. An existence write with `deleted`, the hashes it
observed, is a delete. A register write without `value` is a clear. A file write
without `file` says the entity has no file. Members of an entity that are empty
are omitted. `extended` holds the entries of a known kind that the merge also
keeps whole, each with its hash, so a reader that knows more of one can fold it
again.

Every object of `state` keeps the members a reader does not know: the state
itself, each entity, each register or set, each write, add, removal and `file`,
and each trash item. Where the merge joins two objects into one, such as one
write read from two snapshots, it keeps the greater JSON text under each name.
An object the merge drops, such as an add that a remove takes away, drops its
members with it.

Only a snapshot's writer compacts. It confirms that a file in its directory
holds its last entry, as before an append; if none does, it writes nothing and a
new writer takes over. When a snapshot in its directory already folds that
entry, there is nothing to compact: it writes nothing, deletes as below only
what that snapshot lets it, which a compaction cut short left, and looks first
at that snapshot when it next confirms the entry. Otherwise it folds its own
chain up to that entry into a new snapshot, syncs the snapshot and its
directory, and confirms that the folder holds it. Only then does it delete every
segment in its directory that ends with a seal marker and every line of which a
snapshot in its directory folds, and every snapshot in its directory whose
`folded` list starts the new one's. A segment without a seal marker, such as one
left open by a crash or a copy, is never deleted, and neither is a segment
holding an entry that no snapshot in the directory folds. Just before deleting
each file it confirms that the new snapshot, and the snapshot that folds the
segment, are still in the folder, and that the file still has the length it read
and, for a segment, ends with its seal marker; a file that changed is left.

## Writers

The writers of an install are the directories of its local root. An instance
takes the first, in order of name, that has a readable `head.json`, no
`retired`, and whose lock it gets. It continues that writer only if what it has
read of the writer holds its genesis entry and its recorded head, its history
has one last entry and no fork, and a file in its directory holds that last
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
the instance continue the writer, after the chain's last entry. Until the view
is written, a crash followed by a restore of the folder that takes the commit's
entries takes them from this install's view too.

## Reading

A reader lists `writers/` and reads every file directly in each `writers/<w>/`
other than a swap file, up to 256 MiB of it. A file whose length, modification time and last 34 bytes
are unchanged is not read again; a segment's last 34 bytes are the tab, hash and
LF that end its last line, or those of its seal marker. A segment without a seal
marker that grew is read from the end of its last line on, once the 34 bytes
before that point still end that line; any other changed file is read whole. A
file is a segment when it is empty or its first line can be read, else a
snapshot when it decodes as one; anything else is reported when the library
opens, and read again next time. A snapshot whose `writer` is not `w` is
reported, not used.

A reader places an entry when its `prev` is all zeros, placed, or folded by a
snapshot it has read. An entry whose predecessor it has not is held back, a gap,
reported with the missing hash. Two entries of one writer with one predecessor,
among those placed, folded or ever held back, are a fork: both branches are
merged, and the fork is reported once per predecessor.

Each install keeps what it has placed as a cached view in its local root:
`<genesis>/view.json`, the whole view, and `<genesis>/view.log`, what the view
gained since, one record a line. A new writer writes `view.json` from the view its
instance holds. After each read that kept a snapshot, placed an entry, held one
back or found a file changed, after each append, and when the instance closes,
the instance appends a record of what the view gained to `view.log` and syncs it.
It writes `view.json` again, and then removes `view.log`, when `view.log` has
grown past half of `view.json` and 1 MiB more, when a record could not be
appended or the last one is torn, when the view keeps a new snapshot, which
folds entries `view.json` holds whole, and when the view holds what neither
file does.

Every open starts from the views of the install's writers joined, so what any
instance of the install has shown stays shown: the view of the writer it
continues, then those of the other live writers, then those of the retired
writers that no view it has read holds already. `view.json` is:

```text
{"writers":{"<w>":{
   "snapshots":[<snapshot>, …],
   "entries":[[<json>,"<hash>"], …],
   "strays":[["<hash>","<prev>"], …],
   "forks":[["<prev>",["<branch>","<branch>"]], …]}},
 "files":{"<path>":<file>, …},
 "absorbed":["<genesis>", …]}
```

`snapshots` are the snapshots read, none of whose folded lists starts
another's; `entries` the placed lines no snapshot folds, each after its
predecessor, each line's JSON verbatim with its hash; `strays` the entries ever
held back and never placed, so that a fork with one is found after its file is
gone; `forks` every fork reported. `absorbed` names the retired writers of the
install whose views this one holds. `files` says what the reader last found in
each file of the writers' directories, or for a segment this instance appended
to since, what the append left there when the stat after it found the length it
left, so that a read reads again only the files that changed:

```text
{"len":<n>,"modified":<n>,"tail":"<hex>","segment":{"count":<n>,"first":"<hash>",
  "last":"<hash>","end":<n>,"sealed":<bool>,"lines":[[<json>,"<hash>"], …]}}
{"len":<n>,"modified":<n>,"tail":"<hex>","snapshot":"<hash>"}
```

`len`, `modified` and `tail`, the last bytes in hexadecimal, are the file's
stamp. A segment's lines are `count` entries of the writer's chain from `first`
to `last`, each the successor of the one before; `end` is where its last line
ends; `lines` holds whole the lines the view holds no entry for. A snapshot is
the view's snapshot whose last folded entry is `snapshot`. A file the view
cannot give the lines or the snapshot of is read again.

A record of `view.log` has the same form: `writers` holds what each log gained,
and `files` the files whose records changed, `null` for one that is gone. A
reader replays the records in order onto `view.json`, up to the first that is
torn. Replaying a record twice changes nothing, so a crash between writing
`view.json` and removing `view.log` leaves a view that loads.

The view only grows: a snapshot whose folded list starts a later one's is
replaced by it, and the entries a snapshot folds leave `entries`. It never holds
an entry whose predecessor it does not hold. A view that cannot be read this way
is discarded, and the folder read from scratch.

### After a restore

At each read a reader also places what the folder's files hold without its
cached view. An entry the view holds that no file holds now was taken by a
restore of the folder, or is in a file sync has not brought yet. The reader keeps
showing it and reports the facts it shows that the folder's entries alone would
not: an entity's existence, a register's values, a set's members or a file.
Nothing is republished or dropped until the user chooses:

- **Let go.** The install writes the entries the folder lacks to `let-go.json`
  and shows each writer whose lost entries are all let go as the folder's files
  hold it. When `let-go.json` cannot be written, the entries are let go for now,
  and the next commit, refresh or close writes it again. The cached view keeps
  them. An entry the folder holds again is shown again.
- **Adopt.** The reader commits one intent whose ops make the folder show what
  the reader showed: a `create` or `delete`, a `write` of a value shown (the
  latest, of several the folder lacks), an `add` or `remove` per member and a
  `pin` or `file` op, each replacing or naming what the folder's entries hold.
  Then it lets the entries go. A writer whose own entries the folder lost stops
  first, so the intent is a new writer's.

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
   identities read; opening and refreshing scan every file. A scan that fails
   once a commit's entries are durable does not fail the commit. Until a scan of
   every file succeeds, the paths it would have scanned and what is under them
   are unknown: no file there is bound, reported or a candidate for a move. The
   entities bound there before the commit, and those whose file facts name such
   a path, are bound to no file and shown at their logged path as unscanned.
   Nothing about them is pinned.
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
Opening, reading, refreshing, letting go and scanning write nothing in the
folder; only committing an intent (an undo, a redo, a settlement or an adoption
included), emptying the trash and compaction do.

A commit:

1. Turns the fact changes into ops against the merged state: a register write
   replaces every surviving write of the register, a remove names every live tag
   of its value, a delete observes every live field write of its entity. A key
   the app did not declare, or an entity that is not shown, refuses the intent.
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
them, and says which parts of the instance's own state lag it: the cached view,
`head.json` or `let-go.json` not written, which the next commit, refresh or
close writes again; the scan after it failed, which the next refresh repeats; or
its pending record not removed, which the next write removes. A record whose
intent is logged is not open, so no reader reports it meanwhile. Settling one of
this writer's own effects, adopting, and settling another writer's effect commit
in the same way.

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
6. Remove the pending record.

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
path may also hold the start of its new bytes while a pending record names it.

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
`prev` and no entry after it. A record that is not open is waiting only for its
writer to remove it.

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
appended with what the steps did, and the record is removed. Then every file in
its `tmp/` that no open record places is removed, and so is every empty
`pending/<nonce>.json` opening ignored: a record created in place and cut short
before its bytes landed, before any of its steps ran. Any other record that does
not decode stays. Settling twice ends as settling once.

The steps of a record whose moves copy are found from its markers instead,
since a later step may put bytes back where an earlier one copied them from.
Without a marker, no step is done. Otherwise every step before the last marked
one is done, and that one is done when its source is gone and its destination
is there, unless it is a `place` whose staged file is in the trash. When its
source and destination hold the same bytes, it is done and the source is
removed. When the destination holds fewer of the source's bytes, from the start,
the destination is removed and the step carried out again. Anything else at the
destination stays, and the step stops there.

An open record of any other writer may belong to a live writer elsewhere, or to
this install's own writer from before its local root was lost; a reader cannot
tell them apart. It reports the record, and settles it only when the user asks,
as an intent of its own: to finish, it carries out the remaining steps, skipping
a `to_trash` whose path holds nothing, and logs the facts the record planned; to
roll back, it reverses the done ones; to dismiss, it changes no file. It finds
how far the steps got by checking them from the last, whichever way they move,
since its own settling may have carried them further or reversed some; a
`to_trash` of a record whose moves copy shows done as any other move does. A
library path holding the start of a copy at the destination of the record's last
marked step makes that step the one in progress and goes to the settler's trash
first, as does a library path the last done step left beside its destination. It
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

Each writer undoes only its own intents, most recent first, by committing a
compensating intent whose `reverses` names the entry it reverses. A redo
reverses the latest undo while the writer has committed nothing else since.

- A register write is reversed by writing back the latest value it replaced,
  another writer's included; a clear where there was none.
- An add is reversed by a remove, a remove by an add.
- A create is reversed by a delete, and a delete by a create replacing it.
- A `file` op is reversed by putting back what it changed: the displaced bytes
  from the trash, under a precondition on what the path holds now; the old name;
  or the trash, for a file the intent added.
- `pin` ops are left alone, so undoing an adoption leaves the file.

An undo or redo is refused, naming the writer and entry, where another writer
changed the same thing since: a register no longer holds only what the intent
left, another writer added the same value or removed this add, a delete of the
entity survives, or another writer wrote to an entity being uncreated. It is
refused when the trash no longer holds the bytes it needs. Compaction folds the
entries undo reads, so undo reaches back only to the latest snapshot.

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

The folder keeps, for the life of a writer: every entry, in a segment or folded
in a snapshot; the hash of every folded entry, about 35 bytes each; one segment
per process that crashed or was copied; trash items
until the writer empties them; and the pending records of a writer that will
never run again, settled or not. The local root keeps one directory per writer
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
- **Locks** are Web Locks named `toshokan:/<local root path>/<name>`, held by
  the worker and released when it ends.
- **Hints.** A tab that commits announces its newest entry on the
  `BroadcastChannel` `toshokan:<library>`, and the library's other tabs refresh.
  A page whose folder another program may write also refreshes when it is shown
  or focused, and periodically while it is visible.

