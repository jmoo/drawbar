# Testing

Tests here specify behavior: a failure should say what user-visible behavior,
invariant, or external contract broke, rather than report that output changed.
The standing rules are in
[CONTRIBUTING.md § Tests specify behavior](https://github.com/jmoo/drawbar/blob/master/CONTRIBUTING.md#tests-specify-behavior).

Two suites carry most of the evidence, and both are driven by *files* rather than
by hand-written cases: a specimen joins the format sweep by being readable, and a
capture joins the replay sweep by existing. The public checkout carries enough of
each to run both; the private corpus adds the rest.

## nord-format

Unit tests live inline (`#[cfg(test)] mod tests`) and run on a plain
`cargo test`, alongside `tests/dispatch.rs`, which synthesizes a file for
every registered tag in memory and checks dispatch + round-trip for both
header generations, and the **specimen sweep**, `tests/corpus`: one generated
test per file — container checksum, parse, byte-exact round trip, no
unnameable decoded values, every registry field set to a new value and read
back without moving another, and the file's oracle sidecar
(`<file>.oracle.json`) where it has one. A file joins by being readable; an
oracle by existing beside it.

The sweep always reads `tests/fixtures/` — specimens this crate's own writers
produced, with sidecars saying what was set, committed as the part of the
corpus any checkout can carry.
With `--features corpus` it also reads the private specimen corpus. Three more
suites then run: corpus-backed format and codec behaviors
(`tests/corpus_behaviors.rs`, `tests/codec_behaviors.rs`) and the blind-bit
ledger (`tests/coverage.rs`) — every bit the instrument varies must answer to a
registered field or be listed, by range, as reviewed debt.

```sh
cargo test -p nord-format                       # open suite: unit + dispatch + fixtures sweep

# With the corpus — point at a nord-corpus checkout:
NORD_CORPUS_ROOT=/path/to/nord-corpus \
  cargo test -p nord-format --features corpus

# With nix
nix build .#nord.nord-format-corpus
```

## nord-usb

The integration tests replay real captures through the whole stack and assert the
bytes this crate emits are **the bytes NSM sent** — not merely self-consistent
with its own encoder. No hardware, no platform dependency.

```sh
cargo test -p nord-usb --features replay
```

⚠️ `replay` is not a default feature, so a bare `cargo test -p nord-usb` compiles
the replay tests out and reports a pass having verified none of the wire encoding.
The Nix build enables it via `[package.metadata.nix] testFeatures` in `Cargo.toml`.

### The replay sweep

`tests/replay` is one trial per `*.script` under `tests/scripts`, and — with
`--features corpus` and `NORD_CORPUS_ROOT` — under the private corpus too. A
capture joins the suite by existing; no test is written for it.

Every script is checked for framing. A script whose header declares an **intent**
is also driven: replayed through an exact-match transport, one section per
transaction, each judged against what it said to `expect`, and the whole script
required to be consumed.

```
# source: nord
# device: Nord Electro 5, firmware v2.04 build 592
# intent: program info 7:11
O 0000001200000006000000010000000006a1
…
# intent: program move 7:11 7:12
…
```

`nord … --record <path>` writes that header itself, so a capture made with the CLI
is a complete replay script. The full vocabulary — header keys, the `expect`
values, and the intent → operation table — is in
[`crates/nord-usb/tests/scripts/README.md`](https://github.com/jmoo/drawbar/blob/master/crates/nord-usb/tests/scripts/README.md).
