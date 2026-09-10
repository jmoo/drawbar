# nord-cli

A command-line tool (`nord`) over [`nord-format`](../nord-format) and
[`nord-usb`](../nord-usb): Nord files and the instrument itself from a terminal.

Reading, editing and moving Nord objects, scriptable — data on stdout, everything
else on stderr, so a summary pipes into `grep` unchanged. `inspect`, `verify` and
`edit` work on files; `device`, `program`, `setlist`, `live`, `settings`, `sample`
and `piano` are the protocol's object classes, and `raw --class N` reaches one
that has no noun of its own.

## Usage

```sh
nord inspect patch.ne5p                 # readable summary of a file
nord verify *.ne5p                      # re-encode and check the bytes come back identical
nord program get 7:4 -o patch.ne5p      # read bank 7 slot 4 off the instrument
nord program put patch.ne5p 7:4 --yes   # and write one back
```

Slots are written `BANK:SLOT`, the way the instrument and Nord Sound Manager show
them, both counted from 1. Every mutating command reads the slot first, says what
it will touch, then refuses without `--yes`; off a terminal that is a real dry
run. Close Nord Sound Manager before attaching — it claims the vendor interface
exclusively.

The guide covers every verb, the file formats, editing fields, and the sample and
piano libraries:

- [nord-cli guide](https://jmoo.github.io/drawbar/docs/nord-cli/overview.html)

## Build & run

From the repo root:

```sh
nix run .#nord-cli -- --help
```

Or in the development shell, from `crates/`:

```sh
cargo run -p nord-cli -- inspect patch.ne5p
cargo test -p nord-cli
```

`nix build .#nord-cli` installs the `nord` binary and runs its install check.

## Disclaimer

Not affiliated with, authorized, or endorsed by Clavia DMI AB. "Nord", "Clavia",
and "Electro" are trademarks of Clavia DMI AB, used here only to identify the
hardware these formats come from.
