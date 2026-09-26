# Overview

`nord` works on Nord files and on a connected instrument from the terminal. It
prints data to stdout and everything else to stderr, so its output pipes
cleanly. Every command that changes the instrument says what it is about to
replace and asks before it goes ahead.

```sh
cargo install nord-cli    # or: nix run github:jmoo/drawbar#nord-cli
nord --help
```

## Commands

| Command | Works on |
|---|---|
| `inspect` | Files: print what is in them |
| `verify` | Files: check that they re-encode byte for byte |
| `edit` | Files: change fields in any editable file |
| `device` | The instrument: what is attached and what it holds |
| `program`, `setlist`, `live`, `settings` | Programs, set lists, live slots and settings on the instrument |
| `sample`, `piano` | Sample instruments and piano libraries, on the instrument or as files |

`program`, `setlist`, `sample` and `piano` share the same verbs: `get` and `put`
to transfer, `move`, `rename`, `duplicate`, `delete` and `select` to organize,
`info`, `deps`, `list` and `focus` to look, and `edit`. `live` and `settings`
keep only the verbs that make sense for them. The read-only verbs and `edit` also
take a file in place of a slot. The hidden `raw --class N` reaches an object
class by number, for anything without a command of its own.

## Slots

Slots are written `BANK:SLOT`, counted from 1, the way the instrument shows
them. `7:4` is bank 7, slot 4.

## Output

Color and Unicode appear only on a terminal, and piped output is plain ASCII.
`--color=always`, `--color=never` and `NO_COLOR` override the color choice.
`--yes` answers a confirmation in advance. Off a terminal, a command that would
ask for confirmation fails instead unless you pass `--yes`.

Next: [Files](files.md), [The instrument](instrument.md),
[Editing](editing.md), and [Samples and pianos](libraries.md).
