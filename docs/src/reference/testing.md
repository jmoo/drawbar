# Testing

A failing test should say which behavior or contract broke. The rules are in
[CONTRIBUTING.md](https://github.com/jmoo/drawbar/blob/master/CONTRIBUTING.md#tests-specify-behavior).
Two file-driven sweeps carry most of the evidence: a specimen joins the format
sweep by being readable, and a capture joins the replay sweep by existing.

## nord-format

`cargo test -p nord-format` runs the unit tests, a dispatch test that synthesizes
a file for every registered tag and round-trips it through both container
generations, the Stage body tests, and the specimen sweep over `tests/fixtures`:
files this crate's own writers produced, with sidecars recording what was set.
The sweep fails unless that tree holds a file of every type the reader
dispatches. Each specimen is parsed, round-tripped byte for byte, checked for
unnameable values, and has every registry field set and read back without moving
another.

With `--features corpus`, `NORD_CORPUS_ROOT` names a corpus: any tree of Nord
files. The sweep runs over every file in it the reader recognizes, wherever it
sits. Claims about every file of a kind, such as a Stage 4 selector staying in
panel range or a piano library rebuilding byte for byte, run once per file of
that kind, so a tree without that kind runs none of them. Claims about one file
live in the `<file>.oracle.json` beside it: decoded values, the project a sample
was rendered from, the file another was edited from. A tree without sidecars
still gets the round trips and the per-kind claims. A bit-ownership test flips
every claimed body bit in the fixtures and one file of each container shape, and
requires that it change only the field that claims it, or be refused. `nix build
.#nord.nord-format-corpus` runs the suite over the private corpus, whose
sidecars say the most.

## nord-usb

```sh
cargo test -p nord-usb --features replay
```

The replay tests drive the whole stack from recorded captures and check that the
bytes the crate emits are the bytes Nord Sound Manager sent. `replay` is not a
default feature. Without it, `cargo test -p nord-usb` verifies none of the wire
encoding. The Nix build enables it.

`tests/replay` runs one trial per script under `tests/scripts`, and under the
corpus with `--features corpus`. Every script is checked for framing, and one
that declares an intent is driven through an exact-match transport and judged
against what it said to expect. One that declares none must name the tests that
drive it, or say why nothing does.

```
# source: nord
# device: Nord Electro 5, firmware v2.04 build 592
# intent: program info 7:11
O 0000001200000006000000010000000006a1
```

A read that timed out, a write the device did not accept, and a transport failure
are steps of their own, so a read the script does not expect fails the replay
instead of passing as silence. `nord … --record <path>` writes a complete script.
The step forms, the header keys, the `expect` values and the intent table are
documented in
[`crates/nord-usb/tests/scripts/README.md`](https://github.com/jmoo/drawbar/blob/master/crates/nord-usb/tests/scripts/README.md).

## Everything at once

`nix flake check` and `nix build .#nord.all` run what CI runs. See
[Building from source](building.md).
