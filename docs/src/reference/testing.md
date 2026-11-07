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
another. A piano library or sample instrument, alone or in a bundle, must also
index to the bytes a whole read gives each stroke or zone, reading none of the
audio to find them. A sample's outline must answer as a whole read does, and an
edit saved as a patch must write the bytes a whole edit writes. An edited piano
library streamed from its file must write the bytes the whole library writes,
reading one stroke at a time.

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
corpus with `--features corpus`, and `nord … --record <path>` writes a script.
The script format and what each trial checks are in
[`crates/nord-usb/tests/scripts/README.md`](https://github.com/jmoo/drawbar/blob/master/crates/nord-usb/tests/scripts/README.md).

## drawbar on an instrument

`cargo test -p drawbar` runs headless: the device worker talks to a scripted
instrument, and no test needs hardware. A separate suite in
`crates/drawbar/src/device/hardware.rs` drives drawbar's own send paths against an
attached instrument: a library over a temporary folder, the browser's acts, the
queue, and the device worker over USB. It sends piano libraries and sample
instruments from the files they rest in, replaces an occupied slot, changes a file
on disk while it is being sent, keeps in the library an occupant it could not put
back, and checks that memory stays bounded by the transfer chunk.

The tests write to the instrument, so they are ignored and do nothing unless
`DRAWBAR_HARDWARE=1`. Close drawbar and Nord Sound Manager first, so the interface
is free, and run them one at a time:

```sh
DRAWBAR_HARDWARE=1 cargo test -p drawbar --lib device::hardware -- --ignored --test-threads=1 --nocapture
```

nord-cli reads the source objects, reads back what drawbar wrote, and deletes it
again; `DRAWBAR_HARDWARE_NORD` names its binary when it is not `nord` on the
path. The sources are slots on the instrument:

| Variable | Default | Holds |
|---|---|---|
| `DRAWBAR_HARDWARE_PIANO` | `1:6` | a piano library of a few megabytes |
| `DRAWBAR_HARDWARE_OTHER_PIANO` | `6:3` | another piano library of a few megabytes |
| `DRAWBAR_HARDWARE_SAMPLE` | `1:20` | a sample instrument of more than a megabyte |
| `DRAWBAR_HARDWARE_PROGRAM` | `6:9` | a program that plays a piano and a sample, which the bundle test reads |

Each test writes only to the last vacant slots of bank 1 and deletes only names
it gave, and the last test checks that every class holds what it held when the
run started.

## In the browser

`crates/drawbar/tests/web.rs` runs the browser's library through its worker: in
the private file system and in a folder of it opened as a picked folder is, with
two installs of one library and two tabs of one install, and with a worker that
faults, stalls or loses an answer. toshokan's `backends`, `effects`, `hints` and
`web` suites run its browser backends, in the private file system and in a picked
folder, and the hints that tell a page to look again. wasm-bindgen's test runner
runs them headless; on Linux, Nix runs both crates' suites in Chromium and
Firefox:

```sh
nix build .#nord.browser-tests
```

Each crate declares its browser suites as `browserTests` in its
`[package.metadata.nix]`, and CI's web job runs them. When toshokan or drawbar's
library changes, an optional `safari` job runs them in Safari on macOS too; Safari
cannot pick a folder, so it skips drawbar's picked-folder test. Edge runs them by
hand, with `MSEDGEDRIVER` in place of `CHROMEDRIVER` below.

Elsewhere, from `crates/` in `nix develop`, point the runner at a browser's
driver, with the browser installed:

```sh
CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner \
  CHROMEDRIVER=chromedriver cargo test -p drawbar --target wasm32-unknown-unknown --test web
CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner \
  GECKODRIVER=geckodriver cargo test -p toshokan --features web --target wasm32-unknown-unknown \
  --test backends --test effects --test hints --test web
```

Safari runs them in a window, with `SAFARIDRIVER=/usr/bin/safaridriver`, once
`sudo safaridriver --enable` has allowed remote automation.

`nix build .#toshokan-browser-crash` kills toshokan's library partway through its
power-loss workloads in Chromium and Firefox on Linux, and fills the origin's
quota, and checks that nothing acknowledged is lost or damaged.
`nix run .#toshokan-browser-crash -- --stride 1` runs every case, and adds WebKit
on macOS. `crates/toshokan/spec/README.md` describes the cases.

## Everything at once

`nix flake check` and `nix build .#nord.tests .#nord.packages .#site` run the
public CI gates locally. On Linux, add `.#nord.browser-tests` for declared browser
suites. See [Building from source](building.md).

From `crates/`, `nix develop -c bash ../scripts/test.bash` runs the same native
suite without a sandbox, as CI does. It reads each crate's `testFeatures` from its
manifest. The `release-tests` profile keeps optimization enabled and omits full
link-time optimization; shipped packages keep their shipping profiles.

`scripts/test.bash` then runs drawbar's `store::` and `app::` tests twice more,
with `DRAWBAR_TEST_FOLDER` giving the test folders what a browser's can do:
`browser` renames files but no folder and flushes nothing, as a browser's own
storage or a folder picked in Chrome, and `copying` renames nothing, as a folder
picked in Brave. The default, `desktop`, does what the disk reports.

`nix build .#nord.all-corpus` runs every suite against the pinned private corpus,
and `.#nord.<crate>-corpus` runs one crate's. `.#nord.all-corpus-full` uses the
corpus's R2 tier and needs a seeded store or R2 credentials. `.#nord.corpus` and
`.#nord.corpus-full` are the corpus trees themselves.

A change to toshokan also needs its value-level retention sweep and its browser
crash sweep, which CI does not run. `scripts/evidence.bash` runs all of it at once
and prints each part's wall time: the sweep, every Nix build and every check but
TLC's model check, each crate's tests among them, and on Linux the browser suites
and crash sweep. This full tier gates the change's merge. `--fast` runs
toshokan's own suite and Clippy, for every change in between.
`nix build .#nord.toshokan-spec` runs TLC's model check on its own.

Tests that run long build with `--profile release-tests`:

```sh
cargo test --workspace --profile release-tests
cargo test -p toshokan --profile release-tests --test retention_sweep -- --ignored
```
