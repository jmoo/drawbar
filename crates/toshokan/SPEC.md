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

Nothing in a library path is overwritten or removed in place. An intent's file
effects are a list of steps, each of which moves one file or directory:

| Step                                      | Moves                                   |
| ----------------------------------------- | --------------------------------------- |
| `{"step":"to_trash","path":p,"item":n}`   | `p` to `trash/n`                        |
| `{"step":"place","staged":n,"path":p}`    | `tmp/n` to `p`                          |
| `{"step":"rename","from":p,"to":q}`       | `p` to `q`                              |
| `{"step":"from_trash","item":n,"path":p}` | `trash/n` to `p`                        |
| `{"step":"make_dir","path":p}`            | Creates directory `p`                   |
| `{"step":"remove_dir","path":p}`          | Removes `p` if it is an empty directory |

Paths in steps are library paths: relative to the folder, outside toshokan's
root, and never the folder itself. `tmp/`, `trash/` and `pending/` are those of
the writer the record belongs to.

A save over a file is `to_trash` then `place`; a save where nothing is, `place`;
a delete, `to_trash`; a rename, `rename`; restoring a trash item over a file,
`to_trash` then `from_trash`. A directory moves by one `rename` where the
backend renames directories, otherwise by one `rename` per file, deepest
`remove_dir` first after them.

A writer carries out an intent in this order:

1. Write each new file to `tmp/<nonce>`, sync it, then sync `tmp/`.
2. Check every precondition: a path holds nothing, or a file whose identity is
   the one the writer last read. Check that every trash item a step restores
   is there. On failure, remove the staged files and write nothing more.
3. Write the pending record to `tmp/<nonce>`, sync it, and rename it to
   `pending/<nonce>.json`.
4. Carry out the steps in order. A move first creates the destination's
   directory, renames without replacing, syncs the destination's directory and
   then the source's, so a source's name is gone only once the destination's
   is durable.
5. Append the intent's entry.
6. Remove the pending record.

If a step fails, the rest are not tried: staged files not placed are renamed
to `trash/<their nonce>`, and the entry records the effects that were made.

A pending record is a JSON object:

```json
{"writer":"<w>","entry":<entry>,"label":"Save","steps":[<step>,…],"files":[{"entity":"<e>","path":"a/b.syx","done_after":2}]}
```

`entry` is the intent's entry as planned, whose `prev` is the writer's head when
the record was written. `files` says where each entity's file is once the first
`done_after` steps are done; a `path` of `null` is no file. A reader reads at
most 16 MiB of a record.

### Recovery

A record is open while its writer's log holds the entry named by its entry's
`prev` and no entry after it. A record that is not open is waiting only for its
writer to remove it.

A record is ignored, and reported, when it does not decode, names another
writer, names a path that is not a library path, or its `prev` is not in its
writer's log.

A writer settles its own open records before it next writes, and only those
whose `prev` is the head its local root recorded. It finds how far the steps
got by checking them from the last: the first that shows done ends the done
prefix. `to_trash` shows done when its trash item exists; any other move when
its destination exists and its source does not, or both hold the same bytes.
A `place` whose staged file is in the trash under its own nonce is not done. When
the last done step's source still holds the same bytes as its destination, the
source is removed. The remaining steps are carried out, the entry is appended
and the record removed, as above. Then every file in its `tmp/` that no open
record places is removed.

An open record of any other writer may belong to a live writer elsewhere. A
reader reports it, and settles it only when the user asks, as an intent of its
own: to finish, it carries out the remaining steps; to roll back, it reverses
the done ones. It copies files out of the other writer's `tmp/` and `trash/`
rather than moving them, and moves a library file it displaces into its own
trash. It then appends a `settle` entry naming the writer and record, after which
no reader reports the record. Only the record's writer removes it.

## Trash

`trash/<nonce>` holds bytes an intent of the writer displaced, or new bytes an
intent staged and could not place. An item is listed with the entry whose
`displaced` names it. Only its writer removes an item, and only by emptying:
first every item displaced longer ago than the policy's age, by default 30 days,
then the oldest items until the rest fit the policy's size, by default 1 GiB.
An item no entry names is never emptied.

Emptying a trash is the only way bytes leave the folder.

## Drafts

`<genesis>/drafts/<entity>.json` is the JSON line `{"base":"<identity>"}`, an LF,
then the unsaved bytes. It applies only while the entity's file holds `base`.
