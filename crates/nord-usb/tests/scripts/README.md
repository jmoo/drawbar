# Replay scripts

Recorded exchanges the sweep in `tests/replay` drives — committed so it has a tree to
read in any checkout, with no corpus. A script joins by having the extension; the sweep
walks these directories without knowing what is in them, and the private corpus the same
way under `--features corpus`.

**Protocol framing only.** Bodies and slot names that carry instrument content live in
the private corpus, not here.

```sh
cargo test -p nord-usb --features replay --test replay            # these
NORD_CORPUS_ROOT=/path/to/nord-corpus \
  cargo test -p nord-usb --features corpus --test replay          # and the corpus
```

Every script is checked for two things whatever its header says: it parses, and every
frame's leading length word equals the bytes recorded for it. A script that declares an
`intent` is also **driven** — replayed through an exact-match transport, so the bytes
this crate emits must equal the bytes that were captured, and the whole script must be
consumed.

## Format

A frame is `O <hex>` (host → device) or `I <hex>` (device → host), one per line, and may
carry a trailing `# label`. Any other `#` line is prose unless it reads `# <key>: <value>`
with the key in `[a-z_]+`, which makes it a field. **An unknown lowercase key is an
error**, the same rule the corpus's specimen sidecars follow.

| Key | Scope | Value |
|---|---|---|
| `source` | file | `nsm` — captured from Nord Sound Manager, the oracle · `nord` — recorded by our own CLI · `synthetic` — built by hand |
| `device` | file | free text: model and firmware |
| `trimmed` | file | what was left out of the capture, e.g. `ui-refresh` |
| `note` | file | prose |
| `intent` | section | what the host was doing: `<class> <verb> <args…>` |
| `expect` | section | `ok` (the default) or `err <kind>` |

The file-level keys must precede the first frame. `intent` opens a **section** running to
the next `intent` or to the end of the file: one command is several transactions — a
`put` into an occupied slot is five — so a recording of one command is one script of
several sections, driven in order on one transport.

`expect` names the outcome of its own section:

| `expect` | Passes on |
|---|---|
| `ok` | the operation and its closing exchanges succeeded |
| `err device-status <code>` | the device refused it with exactly that status (`0x15`, `1`) |
| `err unexpected-response` | a reply answered the wrong command |
| `err transport` | the byte pipe failed |
| `err replay` | the script and the code under test contradicted each other |

## Intents

Classes are the CLI's nouns — `program`, `setlist`, `live`, `settings`, `sample`,
`piano`, `device`, and `class-<n>` for one with no noun. Slots are `BANK:SLOT` as the
panel labels them, counting from 1. A name with spaces is `"quoted"`. A file argument is
a path beside the script.

| Intent | Drives |
|---|---|
| `device status` | the inventory sweep, one transaction per class |
| `device geometry` | the partition table, then every partition's banks |
| `device recover` | the two bare frames that release an abandoned session |
| `<class> status` | that class's counters |
| `<class> walk [cap]` | the occupied-slot enumeration, then an `info` per slot |
| `<class> focus` | what the panel has loaded, then an `info` on it |
| `<class> info <at>` | one slot's metadata |
| `<class> deps <at>` | one object's library dependencies |
| `<class> check-address <at>` | the bank/slot bounds check, from the device's geometry |
| `<class> select <at>` | load an object live on the instrument |
| `<class> get <at> [file]` | name a slot, then read it as a container |
| `<class> get-body <at> [file]` | the same, keeping the wire body verbatim |
| `<class> read <at> [file]` | the bare container read, with no `info` first |
| `<class> read-body <at> [file]` | the bare body read |
| `<class> put <file> <at> <name> <stamp>` | write a file into a slot |
| `<class> move <from> <to>` | move, swapping with any occupant |
| `<class> duplicate <from> <to>` | the device-internal deep copy |
| `<class> rename <at> <name>` | rename |
| `<class> delete <at>…` | delete every slot named, in one transaction |

`get` is what the CLI's own verb sends; `read` and `read-body` are the bare transfers it
performs inside a larger operation. A file after a read is compared against what the read
rebuilt, byte for byte. `put`'s name and timestamp are `BEGIN_WRITE` arguments the file
itself does not carry, so the intent states them, and the CLI records the ones it used.

A script with no intent anywhere is still a trial — its framing is checked. A script that
declares an intent on *some* sections is an error: the frames in between would belong to
nothing.

## Writing one

`nord … --record <path>` writes all of this itself: the header, and an `intent` at every
transaction it opens. A capture made that way is a finished golden — drop it in a
directory here or in the corpus and it is a trial.

The hand-built ones under `session/` cover paths no instrument produces on request: a
refused close, a notification flood, a session an earlier run left open. Four of them are
driven by `tests/ops.rs` rather than by an intent, because what they assert is the
session driver's behaviour rather than an exchange.
