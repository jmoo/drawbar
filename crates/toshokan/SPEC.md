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

When the backend can append, a writer appends to its newest segment. Otherwise it
writes a new segment.

## Entries

Every entry carries its version, its intent id and its kind. An entry whose kind
or fields a reader does not know is kept verbatim and ignored by the merge; a
writer that finds one in its own log opens read-only.

The kinds are listed here with their fields. Their JSON encoding is to be
completed.

| Kind          | Fields                                  | Effect on the merged state |
| ------------- | --------------------------------------- | -------------------------- |
| `intent`      | label?, reverses?                       | Starts an intent; `reverses` names the intent it undoes or redoes |
| `create`      | entity                                  | Sets the entity's `exists` register to true |
| `delete`      | entity                                  | Sets the entity's `exists` register to false |
| `field`       | entity, name, value?, prior?            | Writes a last-writer-wins field; no value clears it |
| `set_add`     | entity, name, value                     | Adds a value to an add-wins set, tagged with this entry's version |
| `set_remove`  | entity, name, value, observed           | Removes the adds whose tags are in `observed` |
| `blob_added`  | blob, len                               | This writer added a blob |
| `blob_removed`| blob                                    | This writer's garbage collection removed a blob |

## Snapshots

A snapshot folds one writer's entries up to a named segment, keeping tombstones,
the set-remove observations needed for correctness, and the entries of the
writer's undo window. Its encoding is to be completed.

## Blobs

`blobs/<hash>` holds bytes whose BLAKE3 hash is `<hash>`. A blob is written under
`tmp/<writer>/` and renamed into place, and is never rewritten.

## Journal

A writer records each multi-step file effect under `journal/<writer>/` before its
first step and removes the record after its last. Its encoding is to be completed.
