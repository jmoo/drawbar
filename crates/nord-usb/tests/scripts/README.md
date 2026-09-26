# Replay scripts

Recorded exchanges that the sweep in `tests/replay` drives. They are committed so the
sweep has a tree to read in any checkout, without the corpus. Any file with the `.script`
extension is a script: the sweep walks these directories without knowing what is in them,
and walks the private corpus the same way under `--features corpus`.

Only protocol framing is committed here. Bodies and slot names that carry instrument
content live in the private corpus.

```sh
cargo test -p nord-usb --features replay --test replay            # these
NORD_CORPUS_ROOT=/path/to/nord-corpus \
  cargo test -p nord-usb --features corpus --test replay          # and the corpus
```

Every script is checked for two things whatever its header says: it parses, and every
frame's leading length word equals the number of bytes recorded for it. A script that
declares an `intent` is also driven: it is replayed through an exact-match transport, so
the bytes this crate sends must equal the captured bytes, and the whole script must be
consumed.

## Format

A frame is `O <hex>` (host → device) or `I <hex>` (device → host), one per line, and may
carry a trailing `# label`. Any other `#` line is prose unless it reads `# <key>: <value>`
with the key in `[a-z_]+`, which makes it a field. An unknown lowercase key is an error,
the same rule the corpus's specimen sidecars follow.

| Key | Scope | Value |
|---|---|---|
| `source` | file | `nsm`: captured from Nord Sound Manager, the oracle. `nord`: recorded by this project's CLI. `synthetic`: built by hand. |
| `device` | file | free text: model and firmware |
| `trimmed` | file | what was left out of the capture, e.g. `ui-refresh` |
| `note` | file | prose |
| `intent` | section | what the host was doing: `<class> <verb> <args…>` |
| `expect` | section | `ok` (the default) or `err <kind>`, once per section |

The file-level keys must precede the first frame. `intent` opens a section that runs to
the next `intent` or to the end of the file. One command can take several transactions
(a `put` into an occupied slot takes five), so a recording of one command is one script
of several sections, driven in order on one transport.

`expect` names the outcome of its own section and may sit anywhere inside it, with the
intent or under the frames it judges. A recorder only learns the outcome after the frames
are written, so `nord --record` puts it there, and only when the transaction failed. A
section with no `expect` expects `ok`.

| `expect` | Passes on |
|---|---|
| `ok` | the operation and its closing exchanges succeeded |
| `err device-status <code>` | the device refused it with that status (`0x15`, `1`) |
| `err class-refused <code>` | the device refused a session for the class with that status |
| `err unexpected-response` | a reply answered the wrong command |
| `err unexpected-location` | a reply echoed the wrong bank or slot |
| `err unexpected-partition` | a bank table echoed the wrong partition |
| `err enumeration` | enumeration contradicted geometry or exceeded the host limit |
| `err transport` | the byte pipe failed, or a failure with no kind of its own: bad framing, a bad envelope, an invalid argument |
| `err replay` | the script and the code under test contradicted each other |

## Intents

Classes are the CLI's nouns: `program`, `setlist`, `live`, `settings`, `sample`,
`piano`, `device`, and `class-<n>` for a class with no noun. Slots are `BANK:SLOT` as the
panel labels them, counting from 1. A name with spaces is `"quoted"`. A file argument is
a path beside the script.

| Intent | Drives |
|---|---|
| `device status` | the inventory sweep, one transaction per class |
| `device geometry` | the partition table, then every partition's banks |
| `device recover` | the two bare frames that release an abandoned session |
| `<class> status` | that class's counters |
| `<class> walk` | the occupied-slot enumeration, then an `info` per slot |
| `setlist referrers <at>…` | the set lists pointing at those program slots, walked as `walk` is |
| `<class> focus` | what the panel has loaded, then an `info` on it |
| `<class> info <at>` | one slot's metadata |
| `<class> deps <at>` | one object's dependencies |
| `<class> check-address <at>` | the bank/slot bounds check, from the device's geometry |
| `<class> select <at>` | load an object live on the instrument |
| `<class> get <at> [file]` | an `info`, then a read as a container |
| `<class> get-body <at> [file]` | the same, keeping the wire body as sent |
| `<class> read <at> [file]` | the bare container read, with no `info` first |
| `<class> read-body <at> [file]` | the bare body read |
| `<class> put <file> <at> <name> <stamp>` | write a file into a slot, reserving library space first |
| `<class> move <from> <to>` | move, swapping with any occupant |
| `<class> duplicate <from> <to>` | the device-internal deep copy |
| `<class> rename <at> <name>` | rename |
| `<class> delete <at>…` | delete every slot named, in one transaction |

A walk is bounded by the banks the instrument declares, and a `put` into a library class
reserves storage blocks of the size that partition reports. Both come from the script's
own `device geometry` section where it has one. A script recorded without one falls back
to the committed `device/geometry.script`, replayed on a transport of its own. The tables
are static configuration, so the fixture stands in for the instrument the recording was
taken from. A new recording carries its own.

`get` is what the CLI's own verb sends; `read` and `read-body` are the bare transfers it
performs inside a larger operation. A file named after a read is compared byte for byte
against what the read rebuilt. `put`'s name and timestamp are `BEGIN_WRITE` arguments the
file does not carry, so the intent states them, and the CLI records the ones it used.

A script with no intent anywhere is still a trial, and its framing is checked. A script
that declares an intent on only some sections is an error, because the frames in between
would belong to nothing.

## Writing one

`nord … --record <path>` writes all of this itself: the header, an `intent` for every
transaction it opens, and an `expect` under each one that failed, such as the empty slot
a pre-check names before a `put`, or a rename the library classes refuse. A recording
made that way is a complete replay whether the command succeeded or not; put it in a
directory here or in the corpus and it becomes a trial.

The hand-built scripts under `session/` cover paths no instrument produces on request: a
refused close, a notification flood, a session an earlier run left open. Four of them are
driven by `tests/ops.rs` and not by an intent, because they test the session driver's
behavior, not an exchange.
