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
Each specimen is parsed, round-tripped byte for byte, checked for unnameable
values, and has every registry field set and read back without moving another.

With `--features corpus` and `NORD_CORPUS_ROOT` pointing at a checkout of the
private corpus, the same sweep runs over real files, plus format and codec
behavior suites and a coverage ledger: every bit the instrument varies must
belong to a field or be listed as reviewed debt. `nix build
.#nord.nord-format-corpus` runs it.

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
against what it said to expect:

```
# source: nord
# device: Nord Electro 5, firmware v2.04 build 592
# intent: program info 7:11
O 0000001200000006000000010000000006a1
```

`nord … --record <path>` writes a complete script. The header keys, the `expect`
values and the intent table are documented in
[`crates/nord-usb/tests/scripts/README.md`](https://github.com/jmoo/drawbar/blob/master/crates/nord-usb/tests/scripts/README.md).

## Everything at once

`nix flake check` and `nix build .#nord.all` run what CI runs. See
[Building from source](building.md).
