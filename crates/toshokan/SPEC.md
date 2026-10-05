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
that write.

## Blobs

`blobs/<hash>` holds bytes whose BLAKE3 hash is `<hash>`. A blob is written under
`tmp/<writer>/` and renamed into place, and is never rewritten.

## Journal

A writer records each multi-step file effect under `journal/<writer>/` before its
first step and removes the record after its last. Its encoding is to be completed.
