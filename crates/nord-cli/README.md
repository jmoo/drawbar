# nord-cli

**Your Nord from the terminal.** `nord` inspects, edits and moves the sounds in
Nord files and on a connected instrument, with output you can pipe and commands
you can script.

```sh
nord inspect patch.ne5p                    # what is in a file
nord program get 7:4 -o patch.ne5p         # copy bank 7, slot 4 off the instrument
nord program edit patch.ne5p --set center_panel.gain=96 -o louder.ne5p
nord program put louder.ne5p 7:4 --yes     # and send it back
nord piano build strokes/ --name Marimba -o marimba.npno   # a piano library from WAVs
```

Anything that changes the instrument says what it is about to replace and asks
first. `--yes` answers in advance, for scripts. Data goes to stdout and everything
else to stderr.

```sh
cargo install nord-cli
nix run github:jmoo/drawbar#nord-cli -- --help
```

The [guide](../../docs/src/nord-cli/overview.md) covers every command, and
[What is supported](../../docs/src/getting-started/support.md) says what has been
tested on an instrument. This is alpha software: back up first.

## Building

`nix develop`, then from `crates/`: `cargo run -p nord-cli -- --help` and
`cargo test -p nord-cli`. `nix build .#nord-cli` also runs an install check. See
[Building from source](../../docs/src/reference/building.md).

## Disclaimer

Not affiliated with, authorized, or endorsed by Clavia DMI AB. "Nord", "Clavia",
and "Electro" are trademarks of Clavia DMI AB, used here only to identify the
hardware these formats come from.
