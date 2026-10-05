# toshokan on-disk format

This document specifies everything toshokan writes, precisely enough for a second
implementation to read and write it. The format is unstable while toshokan is a
proof of concept.

All text is UTF-8. All hexadecimal is lowercase. A decimal number has no sign and
no leading zeros, and `0` is written `0`.

## Layout

The app chooses a library folder and a root directory inside it; the default root
is `.toshokan`. toshokan writes only under the root. Every path below is relative
to the root.

| Path                                  | Contents                                   |
| ------------------------------------- | ------------------------------------------ |
| `writers/<writer>/log-<n>.jsonl`      | Segment `n` of a writer's log              |
| `writers/<writer>/snapshot-<hash>.json` | A snapshot of a writer's log             |
| `blobs/<hash>`                        | A blob                                     |
| `journal/<writer>/`                   | A writer's journal of unfinished effects   |
| `tmp/<writer>/`                       | A writer's files before they are renamed into place |

`<writer>` is a writer id. `<n>` is a decimal segment number; a writer's segments
are numbered in increasing order. `<hash>` is the BLAKE3 hash of the file's own
bytes, in 64 hexadecimal digits. A reader ignores any other name in a writer's
directory.

Only writer `w` creates, appends to or removes files under `writers/<w>/`,
`journal/<w>/` and `tmp/<w>/`.

## Identifiers

| Name      | Text form               | Meaning                                         |
| --------- | ----------------------- | ----------------------------------------------- |
| writer id | 32 hexadecimal digits   | 128 bits the app supplies for each writer       |
| entity id | `<writer>:<counter>`    | An entity, allocated from the writer's counter  |
| intent id | `<writer>:<counter>`    | An intent, allocated from the writer's counter  |
| version   | `<lamport>@<writer>`    | When an entry was written                       |
| blob id   | 64 hexadecimal digits   | The BLAKE3 hash of the blob's bytes             |

Counters and Lamport times are decimal `u64`s. Versions are ordered by Lamport
time, then by writer id compared as a 128-bit number. A writer's Lamport time is
one more than the highest it has seen in any log at open or since.

A library path is the file's path relative to the library folder: names joined by
`/`, none empty, `.` or `..`, and none containing NUL. The library folder itself is
the empty path.

## Values

A value is a JSON object with exactly one key, naming its type:

| JSON                     | Value                       |
| ------------------------ | --------------------------- |
| `{"text":"<string>"}`    | Text                        |
| `{"int":<integer>}`      | Signed 64-bit integer       |
| `{"bool":true}`          | Boolean                     |
| `{"ref":"<entity id>"}`  | A reference to an entity    |
| `{"blob":"<blob id>"}`   | A reference to a blob       |

Values are ordered by type in the order of this table, then by contents.

Two field names are toshokan's own: `path` holds an entity's library path as text,
and `content` holds the blob id of the bytes toshokan last wrote or bound for that
file.

## Log segments

A segment is a sequence of lines. Each line is:

```text
<json> TAB <crc> LF
```

`<json>` is one entry as JSON with no tab or newline outside strings. `<crc>` is the
CRC-32 (ISO-HDLC: polynomial `0x04C11DB7`, reflected, initial and final XOR
`0xFFFFFFFF`) of the bytes of `<json>`, in eight hexadecimal digits.

A line whose checksum does not match, or a final line without its LF, ends the
readable segment. Every line before it counts; nothing from it on does.

A writer's segments are numbered from 1. Each session of a writer starts a new
segment, numbered past every segment and snapshot the writer has had. Where the
backend can append, the session appends its later entries to that segment;
otherwise it writes each batch as the next segment. A writer never appends to a
segment an earlier session wrote, so readable lines never follow a torn tail.

## Entries

An entry is one JSON object: `version` holds its version and `intent` its intent
id, both in their text forms, `kind` names its kind, and the kind's members
follow. A writer writes the members in the order below, with no whitespace; a
reader accepts any order. A member marked `?` is omitted when absent.

| `kind`         | Members                                                  |
| -------------- | -------------------------------------------------------- |
| `intent`       | `label?` string, `reverses?` intent id                   |
| `create`       | `entity` entity id                                       |
| `delete`       | `entity` entity id                                       |
| `field`        | `entity` entity id, `name` string, `value?` value, `prior?` value |
| `set_add`      | `entity` entity id, `name` string, `value` value         |
| `set_remove`   | `entity` entity id, `name` string, `value` value, `observed` array of versions |
| `blob_added`   | `blob` blob id, `len` number of bytes                    |
| `blob_removed` | `blob` blob id                                           |

For example, with writer `0000000000000000000000000000000a`, this line sets field
`tag` of entity `…0a:0` to the text `Brass`:

```text
{"version":"3@0000000000000000000000000000000a","intent":"0000000000000000000000000000000a:1","kind":"field","entity":"0000000000000000000000000000000a:0","name":"tag","value":{"text":"Brass"}}	c8b0f391
```

Every intent starts with an `intent` entry, and every entry of an intent carries
its intent id. `reverses` names the intent an undo or redo reverses. A `field`
entry's `prior` is what the writer's merged state held when it wrote; undo
restores it. A `blob_added` entry records that its writer put a blob in the store,
and a `blob_removed` entry that its writer's garbage collection removed it.

A JSON object without a version, an intent id or a string `kind` is unreadable
and ends the segment, as a failed checksum does. An entry whose kind a reader does
not know, or whose kind has a member or a value type the reader does not know, is
kept verbatim and ignored by the merge. A writer that finds one in its own log is
read-only: it neither appends nor compacts.

## Merging

The merged state depends only on the set of entries and snapshots read. The order
of reading does not matter, and reading an entry twice changes nothing.

- **Existence.** Each entity has a register: `create` writes true and `delete`
  writes false, and the entry with the highest version decides. An entity exists
  only while its register is true. A `field` entry does not touch the register,
  so a write concurrent with a delete leaves the entity deleted.
- **Fields.** A field, named by entity and name, holds the value of the `field`
  entry with the highest version. An entry without `value` clears it.
- **Sets.** A `set_add` tags `value` in the set, named by entity and name, with
  the entry's version. A `set_remove` removes the tags in `observed`. A value is a
  member while one of its tags is not removed, so an add the remover had not
  observed survives.
- **Blobs.** For each blob and writer, that writer's `blob_added` or
  `blob_removed` with the highest version says whether the writer still holds
  the blob.
- **Clock.** A writer's next Lamport time is one more than the highest in any log
  it has read.

Two entries share a version only if one is forged. A tie between them goes to the
greater value: absent before present, then values in their order above, `false`
before `true`.

## Snapshots

A snapshot folds one writer's entries up to a segment, keeping tombstones and
set-remove observations, and keeping the entries of the writer's undo window
whole. `writers/<writer>/snapshot-<hash>.json` holds one JSON object and a LF:

| Member     | Contents                                                       |
| ---------- | -------------------------------------------------------------- |
| `through`  | The number of the last segment folded in                       |
| `state`    | The folded state, below                                        |
| `retained` | The undo window: each entry's JSON as a string, in version order |

| `state` member | Records                                                       |
| -------------- | ------------------------------------------------------------- |
| `lamport`      | The highest Lamport time folded                               |
| `exists`       | `{"entity","version","exists"}` for each existence register   |
| `fields`       | `{"entity","name","version","value"?}` for each field          |
| `sets`         | `{"entity","name","value","added","removed"}`, with the value's add tags and removed tags as arrays of versions |
| `blobs`        | `{"blob","len","version","removed"}`, one per blob and writer; the writer is the version's |
| `allocated`    | `{"writer","entity"?,"intent"?}`: the highest entity and intent counters seen of each writer |

Records are sorted by their key, and a reader joins repeated records. A reader
skips the writer's segments numbered `through` or less, applies the state, and
then applies the retained entries and the later segments as it would any entries.
A snapshot whose bytes do not hash to its name is corrupt.

A writer compacts by writing a snapshot of its own snapshots and segments, then
removing its segments numbered `through` or less and its other snapshots. Where
the backend renames files, it writes the snapshot under `tmp/<writer>/` and
renames it into place. A reader that finds more than one snapshot of a writer
joins them: the highest `through`, the join of their states, and the union of
their retained entries. A read-only writer does not compact.

## Undo and redo

A writer undoes and redoes only its own intents, by appending a new intent whose
`intent` entry names the reversed intent in `reverses`. Replaying the writer's
intents in version order gives two stacks. An intent that reverses the top of the
undo stack is an undo and moves to the redo stack; one that reverses the top of
the redo stack is a redo and moves back. Any other intent that writes a fact goes
on the undo stack and empties the redo stack.

Reversing an intent writes, newest first: `delete` for its `create`, `create` for
its `delete`, a `set_remove` observing only its own tag for its `set_add`, a
`set_add` for its `set_remove`, and for each field it wrote, the prior of its
first write to that field. A field's undo is refused when the field's deciding
write is neither the intent's last write to it nor an undo or redo that restored
that write. An intent of only `blob_added` and `blob_removed` entries is not
undone.

Reversing an intent also reverses its files. For each entity whose `content` it
changed, the file at the entity's `path`, or at the path the intent cleared, is
restored: a save of the earlier blob when there was one, otherwise a delete. Either
expects the file to hold exactly the intent's blob, or no file where the intent
cleared `content`. Then each `path` the intent changed from one path to another is
renamed back. The reversal is refused when the earlier blob has no `blob_added`
that its writer has not since removed.

## Intents

An intent is one batch of entries under one intent id, its `intent` entry first.
The library writes these:

| Intent          | Entries after `intent`                                             |
| --------------- | ------------------------------------------------------------------ |
| create          | `create` with a new entity id                                      |
| set             | `field`, never `path` or `content`                                 |
| add             | `set_add`                                                          |
| remove          | `set_remove` observing the value's live tags in the writer's merged state |
| delete          | `delete`                                                           |
| bind            | `field` `path` and `content`: the file's path and the hash of its bytes |
| save, delete file, rename, move tree | the effect's entries, below                   |
| undo, redo      | the reversing entries, then their effects' entries                 |

An intent with a file effect is journaled whole before the effect's first step, so
its entries reach the log only once its files have changed. Garbage collection and
the recovery of staged bytes write intents of their own.

## Blobs

`blobs/<hash>` holds bytes whose BLAKE3 hash is `<hash>`. A writer stages new
bytes as `tmp/<writer>/<hash>`, syncs them, and renames them into place. A blob is
never rewritten. A library file that an effect displaces is renamed into `blobs/`
under the hash of its contents; when the store already holds that hash, the file is
removed instead.

A writer logs `blob_added` for every blob it adds or displaces. The new bytes of a
save are not kept in the store: `content` names them while they are the library
file. Garbage collection is per writer: writer `w` may remove a blob only when

- `w`'s latest `blob_added` or `blob_removed` entry for it is an add,
- no field or set member of an existing entity holds it,
- no entry a reader applies one by one (a snapshot's retained entries and the
  segments after it) holds it as a value, prior or `blob_added`, nor names an
  entity with a field or set member that holds it, and
- no other writer's latest entry for it is an add.

It removes such blobs oldest first, by the version of its add, until its remaining
adds total at most its byte budget. It removes the files first, then logs
`blob_removed` for each under a new intent, so a removal that the log missed is
logged by the next collection. When a save finds the disk full, the writer
collects with a budget of zero, then compacts away its undo window and collects
again, before it refuses the save.

## File effects

An effect changes the library and logs what it did under the intent, after the
intent's own entries:

| Effect     | Library                                         | Entries |
| ---------- | ----------------------------------------------- | ------- |
| save       | new bytes at the path; the old file into blobs  | `blob_added` for the old file; `field` `path` and `content` where they change |
| delete     | the file into blobs                             | `blob_added`; `field` `path` and `content` cleared |
| rename     | the file renamed                                | `field` `path` |
| move tree  | the directory and everything in it renamed      | `field` `path` for each entity bound under it |

Each `field` entry's `prior` is the value the writer's merged state held. Before
any step, an effect checks its precondition. A save or delete expects one of: no
file at the path; the fingerprint the writer last read, compared by length, then by
hash when both sides have one, then by an equal modification time; or a file whose
bytes hash to a given blob id. A rename or move refuses a destination that exists.
A refused effect writes nothing.

A rename syncs the destination directory before the source directory, so a crash
between the two leaves the entry under both names rather than under neither.

## Journal

Before the first step of an effect, writer `w` creates `journal/<w>/<n>.json`,
where `<n>` is the counter of the effect's intent id, and syncs it and its
directory. After the effect's entries are in the log, the writer removes the file.

The file is one JSON object with exactly these keys:

| Key       | Value                                                        |
| --------- | ------------------------------------------------------------ |
| `intent`  | The intent id                                                |
| `step`    | The step, below                                              |
| `entries` | An array of strings, each an entry's JSON as a log line holds it: the intent's entries, stamped before the first step, which the log gains when the step finishes |

A stored blob is `{"blob":<blob id>,"len":<integer>}`. A step is one of:

| Step                                                     | Files |
| -------------------------------------------------------- | ----- |
| `{"save":{"path":..,"new":<stored>,"old":<stored or null>}}` | Put the bytes staged as `tmp/<w>/<new blob>` at `path`, which held `old` or no file |
| `{"delete":{"path":..,"old":<stored>}}`                  | Move the file at `path`, holding `old`, into blobs |
| `{"move":{"from":..,"to":..}}`                           | Rename a file, or a directory where the backend renames directories |
| `{"move_files":{"from":..,"to":..,"files":[..],"dirs":[..]}}` | Create `dirs` under `to`, move each of `files`, then remove the empty `dirs` under `from`; both lists are relative, and `dirs` holds `""` for the directory itself |

### Recovery

At open, writer `w` settles each record in order of `<n>`. It brings the files to
the step's end from whatever state they are in:

| Step       | Finished when                                    | Otherwise |
| ---------- | ------------------------------------------------ | --------- |
| save       | `path` holds `new`                               | With the staged file present: rename it into place if `path` is empty, or move `old` into blobs first if `path` holds `old`. Anything else at `path` rolls back. |
| delete     | `path` is empty and blobs holds `old`            | Move the file into blobs if it holds `old`; anything else rolls back. |
| move       | only `to` exists                                 | Rename if only `from` exists; remove `from` if both are files with the same bytes; anything else rolls back. |
| move_files | always                                           | Each file moves if only its source exists, and its source is removed if both hold the same bytes; a file in conflict stays where it is. |

When the step finishes, `w` appends the record's entries with their recorded
versions; appending an entry twice changes nothing in the merge. When it rolls
back, `w` leaves the library as it found it, moves any staged bytes into blobs, and
appends under the record's intent an `intent` entry with no members and a
`blob_added` for each of the step's bytes now in blobs, or nothing when there are
none. Either way it then removes the record.

Last, `w` moves every file left in `tmp/<w>/` whose name is a blob id into blobs,
logging `blob_added` for each first, under a new intent. Any other file there is a
snapshot a compaction had not yet renamed into place. Recovery may itself be
interrupted and repeated.

A writer that is read-only leaves its journal unrecovered.
