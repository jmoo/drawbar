# toshokan on-disk format

This document specifies everything toshokan writes, precisely enough for a second
implementation to read and write it. The format is unstable while toshokan is a
proof of concept. Sections marked _To be specified_ are being written with the
code that implements them.

All text is UTF-8. All hexadecimal is lowercase. JSON is written without
insignificant whitespace.

## Layout

The app chooses a library folder and the name of a root directory inside it,
such as `.drawbar`. In the folder, toshokan writes only under that root. Every
path below is relative to it.

| Path                                  | Contents                                   |
| ------------------------------------- | ------------------------------------------ |
| `writers/<w>/<segment>.jsonl`         | A segment of writer `w`'s log              |
| `writers/<w>/snapshot-<nonce>.json`   | A snapshot of writer `w`'s log             |
| `writers/<w>/pending/<nonce>.json`    | A journal record of an unfinished effect   |
| `writers/<w>/trash/<nonce>`           | Bytes an intent of `w` displaced           |
| `writers/<w>/tmp/<nonce>`             | A file `w` is staging                      |

Only writer `w` creates, appends to, renames or removes anything under
`writers/<w>/`. There is no file at the level of the library.

Names of segments and snapshots are advisory. A reader reads every file directly
in `writers/<w>/`, whatever its name, as a segment or a snapshot by its contents,
so a sync client's conflicted copy is read like any other file.

Each install also keeps a local root of its own, never synced, per library:

| Path                          | Contents                                     |
| ----------------------------- | -------------------------------------------- |
| `<genesis>/head.json`         | The last entry this writer wrote             |
| `<genesis>/view.json`         | The cached view                              |
| `<genesis>/drafts/<entity>.json` | An unsaved edit                           |
| `<genesis>/lock`              | Held while an instance writes as this writer |
| `<genesis>/retired`           | Empty; the writer is never written again     |

`<genesis>` is the hash of the writer's genesis entry. The directory is created
only after that entry is durable in the folder, so the directories of the local
root are the install's pool of writers.

`head.json` is `{"writer":"<writer id>","head":"<entry hash>"}`, the last entry
the writer made durable in the folder. A file in the local root that is replaced,
such as `head.json` or `view.json`, is first written beside it as
`<name>.next` and synced; then `<name>` is removed and `<name>.next` renamed to
it. A reader takes `<name>`, or `<name>.next` when `<name>` is missing.

## Identifiers

Every identifier is 128 bits written as 32 hexadecimal digits.

| Name        | Meaning                                                          |
| ----------- | ---------------------------------------------------------------- |
| writer id   | A writer, random                                                 |
| entity id   | An entity, random                                                |
| segment     | A segment's name, random                                         |
| nonce       | The name of a snapshot, pending record, trash item or staged file, random |
| entry hash  | An entry's id, link and checksum, below                          |
| identity    | What the app's identity function says a file holds               |

References between entries, set tags, removes and replaced writes name entry
hashes.

A clock reading is the JSON array `[wall_ms, counter]`: milliseconds since the
Unix epoch and a counter within the millisecond. Readings order entries for
display only, by reading and then by writer id.

## Segments

A segment is a sequence of lines. Each line is:

```text
<json> TAB <hash> LF
```

`<json>` is one entry, a JSON object, with no tab or newline. Its `prev` member is
the hash of the entry before it in the writer's chain, or 32 zeros for the
writer's genesis entry. `<hash>` is the first 16 bytes of the BLAKE3 hash of the
16 bytes of `prev` followed by the bytes of `<json>`.

For example, the JSON `{"prev":"0…0"}` makes the line:

```text
{"prev":"00000000000000000000000000000000"}	21fbde4eb554d0b73a2edf6821643616
```

A line longer than 1 MiB, a line whose hash does not match, a final line without
its LF, or a line starting with a zero byte ends the readable part of the file
for now. Every line before it counts; a later read of the same file may get
further.

A writer appends to one segment per process, named at random when the process
first appends. It deletes only segments its own process opened and sealed.

## Entries

An entry is the JSON of one line: an object whose first members are

| Member | Value                                          |
| ------ | ---------------------------------------------- |
| `prev` | The hash of the entry before it                |
| `at`   | Its clock reading                              |
| `kind` | `"genesis"`, `"intent"` or `"settle"`          |

followed by the members of its kind. A reader ignores members it does not know
and keeps the line, so they survive. An entry of another kind, without `kind`, or
whose members do not decode, is kept and placed as an unknown entry; so is a
line whose JSON has no readable `at`, because it still links the chain.

A **genesis** entry starts a writer's chain, with `prev` all zeros:

| Member   | Value                                          |
| -------- | ---------------------------------------------- |
| `writer` | The writer's id                                |
| `label`  | The name other writers show for it             |

A genesis entry after another entry is unknown.

An **intent** entry logs one committed intent:

| Member      | Value                                                   |
| ----------- | ------------------------------------------------------- |
| `label`     | What the user did                                       |
| `ops`       | The fact changes, in order                              |
| `displaced` | Bytes it moved into the writer's trash; omitted if none |
| `reverses`  | The entry it undoes or redoes; omitted if none          |

Each op is an object whose `op` member names it:

| `op`     | Members                                  | Meaning                                  |
| -------- | ---------------------------------------- | ---------------------------------------- |
| `create` | `entity`, `replaces`                     | The entity exists                        |
| `delete` | `entity`, `replaces`, `observed`         | The entity is deleted                    |
| `write`  | `entity`, `key`, `value`, `replaces`     | A register holds `value`; cleared without it |
| `add`    | `entity`, `key`, `value`                 | A set gains `value`                      |
| `remove` | `entity`, `key`, `tags`                  | A set loses the adds `tags` names        |
| `file`   | `entity`, `file`, `replaces`             | The entity's file; none without `file`   |

`replaces`, `observed` and `tags` are arrays of entry hashes. A `value` is the
app's JSON, kept byte for byte; `"value":null` writes `null`. A `file` is
`{"path","identity","len"}` with an optional `modified`. Each member of
`displaced` is `{"item","from","identity","len"}`: the trash item's nonce, the
library path the bytes left, their identity and length. An op of another name,
or missing a member, is kept and unknown.

For example:

```json
{"prev":"…02","at":[3,0],"kind":"intent","label":"Tag","ops":[{"op":"add","entity":"…0e","key":"tags","value":"x"},{"op":"write","entity":"…0e","key":"origin","replaces":["…01"]}]}
```

A **settle** entry records that this writer settled another writer's unfinished
effect with the user's consent:

| Member    | Value                                              |
| --------- | -------------------------------------------------- |
| `writer`  | The writer whose pending record it settles         |
| `record`  | The record's nonce                                 |
| `outcome` | `"finished"`, `"rolled-back"` or `"dismissed"`     |

## Writers

The writers of an install are the directories of its local root. An instance
takes the first, in order of name, that has a readable `head.json`, no
`retired`, and whose lock it gets. It continues that writer only if what it has
read of the writer holds its genesis entry and its recorded head, its history
has one last entry and no fork, and a file in its directory holds that last
entry now. Otherwise it
creates `retired`, releases the lock, and writes as a new writer from its next
write: a copied local root, whose history another instance also continues, and a
folder restored from an older copy, which no longer holds the writer's last
entry, each start a new writer.

A new writer creates its first segment holding its genesis entry and syncs it,
its directory and every directory above it. Only then does it create its
directory in the local root, take the lock and write `head.json`.

Before each append, a writer confirms that the folder holds its last entry: its
open segment has the length this process left it, or, with no segment open, a
file in its directory holds that entry. If not, it writes nothing and is
replaced by a new writer. After the append is synced it writes `head.json`. An
append that fails seals the segment, so nothing follows a torn line.

## Reading

A reader lists `writers/` and reads every file directly in each `writers/<w>/`,
up to 256 MiB of it. A file is a segment when it is empty or its first line can
be read, else a snapshot when it decodes as one; anything else is reported and
read again next time. A snapshot whose `writer` is not `w` is reported, not used.

A reader places an entry when its `prev` is all zeros, placed, or folded by a
snapshot it has read. An entry whose predecessor it has not is held back as a
gap. Two entries of one writer with one predecessor, among those placed, folded
or ever held back, are a fork: both branches are merged, and the fork is
reported once per predecessor.

Each install keeps what it has placed as a cached view in `<genesis>/view.json`
of its local root:

```text
{"writers":{"<w>":{
  "snapshots":[<snapshot>, …],
  "entries":["<json>\t<hash>", …],
  "strays":[["<hash>","<prev>"], …],
  "forks":[["<prev>",["<branch>","<branch>"]], …]
}}}
```

`snapshots` are the snapshots read, none of whose folded lists starts
another's; `entries` the placed lines no snapshot folds, each after its
predecessor; `strays` the entries ever held back and never placed, so that a fork
with one is found after its file is gone; `forks` every fork reported. The view
only grows: a snapshot whose folded list starts a later one's is replaced by
it, and the entries a snapshot folds leave `entries`. It never holds an entry
whose predecessor it does not hold. A view that cannot be read this way is
discarded, and the folder read from scratch.

## Merging

_To be specified:_ multi-value registers with grow-only replaced sets,
observed-remove sets, existence, and how unknown kinds and fields are kept.

## Snapshots

`snapshot-<nonce>.json` is one JSON object, without a final newline:

| Member   | Value                                                         |
| -------- | ------------------------------------------------------------- |
| `writer` | The writer's id                                               |
| `label`  | The writer's label                                            |
| `at`     | The clock reading of the last folded entry                    |
| `folded` | The hash of every entry it folds, from the genesis entry on, each the successor of the one before it |
| `state`  | The merged state of those entries                             |

Members a reader does not know are kept. A reader refuses a `folded` list that is
empty or repeats a hash.

Only a snapshot's writer compacts. It folds its own chain up to its last entry
into a new snapshot, syncs the snapshot and its directory, and confirms the
folder holds it. Only then does it delete the segments its process sealed every
line of which the snapshot folds, and every snapshot in its directory whose
`folded` list starts the new one's. Segments left open by a crash or a copy, and
the branches of another instance, are never deleted.

## File effects and pending records

_To be specified._

## Trash

_To be specified._
