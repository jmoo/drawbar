# Building from source

Everything goes through Nix, so a checkout needs nothing else installed.

```sh
git clone https://github.com/jmoo/drawbar && cd drawbar
nix run .#drawbar                 # the desktop app
nix run .#drawbar-web             # the browser build, served with this guide beside it
nix run .#nord-cli -- --help      # the nord command
nix build .#site                  # drawbar.app's layout, from this checkout
nix build .#docs                  # this guide
```

`nix run .#drawbar-web` serves on <http://127.0.0.1:8080/>. Set `DRAWBAR_PORT`
for another port.

## With Cargo

`nix develop` opens a shell with the toolchain. Run Cargo from `crates/`, because
`crates/.cargo/config.toml` carries a flag the wasm build needs, and Cargo finds
it by walking up from the working directory.

```sh
nix develop
cd crates
cargo run -p drawbar
cargo run -p nord-cli -- --help
cargo test --workspace
```

## The browser build by hand

`nix build .#drawbar-web` does all of this in one step. By hand, from `crates/`:

```sh
cargo build -p drawbar --lib --target wasm32-unknown-unknown --release
nix run --inputs-from .. nixpkgs#wasm-bindgen-cli -- \
  --target web --out-dir drawbar/pkg \
  target/wasm32-unknown-unknown/release/drawbar.wasm
cd drawbar && python3 -m http.server 8000
```

`--lib` is required. The crate also has a binary target of the same name, and
if that one wins, the wasm module exports nothing. `wasm-bindgen-cli` must also
match the `wasm-bindgen` version in `Cargo.lock`. `--inputs-from ..` takes it
from the flake's pinned nixpkgs, which matches.

Serve over `http://localhost`, which counts as a secure context. WebUSB and the
module import both fail from `file://`. This plain server has no guide beside the
app, so Help ▸ User guide finds nothing there. `nix run .#drawbar-web` serves
both.

`nix build .#drawbar-web` also writes the version and the module's byte count
into the page. Built by hand it has neither, so the loading screen shows no
version and measures the download against the server's `Content-Length`, or not
at all when the server compresses it.

## Checks

`nix flake check` runs the formatting check, Clippy with warnings denied, and
the tests of `scripts/bump.bash`. `nix build .#nord.all` builds every crate with
its tests, the cross targets, the browser build and this guide. CI runs both.
`nix fmt` formats everything. [Testing](testing.md) describes the suites, and
[Contributing](contributing.md) the house rules.
