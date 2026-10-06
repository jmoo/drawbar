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

`<genesis>` is the hash of the writer's genesis entry. The directory is created
only after that entry is durable in the folder, so the directories of the local
root are the install's pool of writers.

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

_To be specified:_ the members of each entry kind (`genesis`, `intent`, `settle`)
and of each op.

## Reading

A reader places an entry when its predecessor is placed or folded by a snapshot
it has read. An entry whose predecessor it has not is held back as a gap. Two
placed entries with one predecessor are a fork: both branches are merged, and the
fork is reported once.

_To be specified:_ the cached view.

## Merging

_To be specified:_ multi-value registers with grow-only replaced sets,
observed-remove sets, existence, and how unknown kinds and fields are kept.

## Snapshots

_To be specified:_ a snapshot lists every entry it folds, in chain order from the
genesis entry, with the folded state.

## File effects and pending records

_To be specified._

## Trash

_To be specified._
