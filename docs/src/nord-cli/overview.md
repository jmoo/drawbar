# Overview

`nord` is a command-line tool over `nord-format` and `nord-usb`.

| Command | What it does |
|---|---|
| `inspect` | Decode file(s) and print a readable summary |
| `verify` | Re-encode file(s) and check the bytes come back identical |
| `edit` | Change fields inside any editable file, whatever format it holds |
| `device` | The instrument itself — what is on the bus, and what it holds |
| `program` | Programs on the instrument (object class 4) |
| `setlist` | Set lists on the instrument (object class 5) |
| `live` | The three Live slots (object class 6) |
| `settings` | The global settings singleton (object class 7) |
| `sample` | Sample instruments — the library (object class 3), or `.nsmp` files |
| `piano` | Piano libraries — the library (object class 1), or `.npno` files |
| `raw` | Hidden: the same verbs, addressed by class number |

`inspect`, `verify` and `edit` work on files. The other nouns are the protocol's
object classes and normally talk to an attached instrument — but the read-only
verbs (`get`, `info`, `deps`) and each noun's `edit` also take a file in place of
a slot. `program`, `setlist`, `sample` and `piano` share one verb vocabulary:

```
get put                                transfer
move rename duplicate delete select    organization
info deps list focus                   interrogation
edit                                   content
```

Slots are written **`BANK:SLOT`**, the way the instrument and Nord Sound Manager
show them. `7:4` is bank 7, slot 4, both counted from 1. (`7-4` also parses.)

- **Data on stdout, everything else on stderr.**
- **Color and unicode only on a terminal.** `--color=auto|always|never`;
  `NO_COLOR` forces color off.
- **A pipe is non-interactive.** On a terminal a destructive command asks for
  confirmation; off one, a missing `--yes` is an error rather than a prompt.

Every command and its options are in
[`crates/nord-cli/README.md`](https://github.com/jmoo/drawbar/blob/master/crates/nord-cli/README.md).
