# nord-web-demo

A browser page that drives [`nord-usb`](../nord-usb)'s WebUSB backend against a real
instrument. It exists to answer one question: **does the WebUSB transport work on
hardware?** Nothing in it is a product.

> [!NOTE]
> This page has verified the read-only path on hardware (Chrome on macOS:
> inventory and object info). Writes and multi-chunk bulk reads remain
> unexercised over WebUSB.

**Read-only by construction.** The page runs `op::inventory` and, if you name a slot,
`op::info`. It never escalates a session to `ReadWrite`, so no delete/move/rename/write
is reachable from it.

## Build

Everything below runs from `crates/`. The `--target wasm32-unknown-unknown` builds
**must** be run from here or below: `crates/.cargo/config.toml` supplies
`--cfg=web_sys_unstable_apis`, without which WebUSB does not exist in `web-sys`, and
Cargo only finds that file by walking up from the working directory.

```sh
cd crates
nix develop /path/to/lab            # cargo, rustc, and lld (the wasm linker)

cargo build -p nord-web-demo --target wasm32-unknown-unknown

nix run nixpkgs#wasm-bindgen-cli -- \
  --target web \
  --out-dir nord-web-demo/pkg \
  target/wasm32-unknown-unknown/debug/nord_web_demo.wasm
```

⚠️ **`wasm-bindgen-cli` and the `wasm-bindgen` crate must be the same version.** The
CLI refuses a module built by any other one ("rust wasm file schema version… but this
binary schema version…"). nixpkgs currently ships **0.2.121**, so this crate pins
`wasm-bindgen = "=0.2.121"`, which pins the whole workspace lock:

```sh
grep -A2 'name = "wasm-bindgen"' Cargo.lock
nix run nixpkgs#wasm-bindgen-cli -- --version
```

If those two ever disagree, move the pin in `nord-web-demo/Cargo.toml` to whatever the
CLI reports and re-run `cargo build`. `wasm-pack` (`nix run nixpkgs#wasm-pack -- build
--target web nord-web-demo`) is the fallback: it downloads a matching CLI itself, at
the cost of not being sourced from the flake.

## Serve

```sh
cd nord-web-demo && python3 -m http.server 8000
```

Then open <http://localhost:8000/>. `localhost` counts as a secure context, so WebUSB
is available without TLS. `file://` is **not** — the module import and WebUSB both
fail there.

## Run it

1. **Close Nord Sound Manager.** It holds the vendor interface, and Chrome cannot
   claim an interface another process already has.
2. Chrome or Edge. Firefox and Safari have declined WebUSB.
3. Leave the slot box blank for the first attempt, or type a slot in the panel's
   one-indexed numbering (`7:4` = bank 7, program 4).
4. Click **Connect and scan** and pick the instrument in Chrome's chooser. Nothing
   happens before that click — `requestDevice()` needs a user gesture.

A working run prints the device name, then one line per object class:

```
Nord Electro 5 — 0ffc:0027

pianos      29 items,  61.7% used (…/… blocks)
samples    139 items,  …
programs   379 items,  …% used (53439/56400 blocks, 400 slots)
set lists    … items,  …
```

and, with a slot named, one more line with that slot's name, format tag, version, body
length and CRC-32.

The page releases the interface when it finishes, so NSM and `nord-cli` can have the
device back without closing the tab. A run that fails mid-way does not — reload the
page, or replug, if the next attempt cannot claim.

## When it goes wrong

Open the console; every failure is also printed into the page.

| What you see | Most likely cause |
|---|---|
| No device in the chooser | Instrument asleep or on a charge-only cable; the filter is vendor `0x0ffc`. |
| `SecurityError` on claim | Another process (NSM) holds the interface. |
| `NotFoundError` on a transfer | Endpoint numbers wrong for this instrument — the transport assumes IN 2 / OUT 3. |
| Hangs after one exchange | A read is waiting for a short packet that never came. See the `read` hazard note in `nord-usb/src/transport/web.rs`. |
| `device exposes no vendor-specific interface` | Chrome listed a device whose class-`0xff` interface is missing or hidden. |
