# Build and run

The published browser build is at <https://drawbar.app/>, with this guide under
<https://drawbar.app/docs/>. Nothing needs installing to use it.

Nix builds and runs both targets without anything else on the machine:

```sh
nix run .#drawbar            # the desktop app
nix run .#drawbar-web        # drawbar in the browser
```

`nix run .#drawbar-web` serves the browser build with this guide beside it on
<http://127.0.0.1:8080/> and opens it; set `DRAWBAR_PORT` to use another port.
`nix build .#site` produces the same tree: the app at the root and the guide under
`docs/`.

From outside a checkout, `github:jmoo/drawbar#` replaces `.#`, as on the
[Install](../getting-started/install.md) page. The rest of this page is the build
from source.

## Build and run: native

```sh
nix develop            # from the repo root
cd crates
cargo run -p drawbar
```

`cargo test -p drawbar` runs the crate's own suite from the same place.

## Build and serve: web

`nix build .#drawbar-web` produces the whole servable bundle, the bound wasm module
beside `index.html`, in one step. The manual path below is the same build, spelled
out.

Everything below runs from `crates/`. The `--target wasm32-unknown-unknown` builds
**must** be run from here or below: `crates/.cargo/config.toml` supplies
`--cfg=web_sys_unstable_apis`, without which WebUSB does not exist in `web-sys`,
and Cargo only finds that file by walking up from the working directory.

```sh
nix develop            # from the repo root
cd crates

cargo build -p drawbar --lib --target wasm32-unknown-unknown --release

nix run --inputs-from .. nixpkgs#wasm-bindgen-cli -- \
  --target web \
  --out-dir drawbar/pkg \
  target/wasm32-unknown-unknown/release/drawbar.wasm

cd drawbar && python3 -m http.server 8000
```

Then open <http://localhost:8000/>. `localhost` counts as a secure context, so
WebUSB is available without TLS; `file://` is **not**, and the module import and
WebUSB both fail there. This server has no guide beside the app, so **Help ▸ User
guide** finds nothing; use `nix run .#drawbar-web` for that.

> **Important.** `--lib` is not optional. The crate also has a binary target of
> the same name, so building both for wasm writes two different modules to the
> same `target/wasm32-unknown-unknown/*/drawbar.wasm`. Cargo warns about the
> collision and does not define which one survives; when it is the binary's stub
> `main`, `wasm-bindgen` emits a package that exports nothing.

⚠️ `wasm-bindgen-cli` must be the exact version of the workspace's `wasm-bindgen`
pin, because the CLI refuses a module built by any other. `--inputs-from ..` takes
the CLI from the flake's own locked nixpkgs, which the Nix build uses and the pin in
`drawbar/Cargo.toml` matches. To check:

```sh
grep -A2 'name = "wasm-bindgen"' Cargo.lock
nix run --inputs-from .. nixpkgs#wasm-bindgen-cli -- --version
```

## Browser support

| Browser | Works |
|---|---|
| Chrome | yes |
| Edge | yes |
| Firefox | no: WebUSB declined |
| Safari | no: WebUSB declined |

**Close Nord Sound Manager before connecting.** It claims the vendor interface
exclusively, and nothing else (drawbar, `nord-cli` or Chrome) can attach alongside
it.
