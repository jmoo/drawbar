# drawbar

An [egui](https://github.com/emilk/egui) app over
[`nord-format`](../nord-format) and [`nord-usb`](../nord-usb) — everything
[`nord-cli`](../nord-cli) can do that is worth a window, reachable from a browser
tab or a desktop one.

![drawbar with a Nord Electro 5 attached and a piano library open in its trim-to-fit editor](../../docs/src/assets/screenshot.png)

It opens at <https://drawbar.app/>. The window is a dock shell around your sounds:
a browser tree over this computer, an attached instrument's folders, kinds and
tags; a Library table over both places; a Keyboard tab for the instrument; and an
inspector. Anything you open becomes a document under one header, with an editor
for Nord Electro 5 and Stage programs, set lists, sample instruments, Sample Editor
projects and piano libraries. A send waits in a send queue, where you can review
what it replaces, until you send the queue.

## Usage

```sh
nix run .#drawbar            # the desktop app
nix run .#drawbar-web        # drawbar in the browser
```

The user guide walks through it:

- [Using drawbar](https://drawbar.app/docs/drawbar/overview.html): the window,
  this computer, the instrument and the send queue.
- [Editing](https://drawbar.app/docs/drawbar/editing.html),
  [Samples](https://drawbar.app/docs/drawbar/samples.html) and
  [Pianos](https://drawbar.app/docs/drawbar/pianos.html): the document editors.
- [Build and run](https://drawbar.app/docs/drawbar/build.html): the native and web
  builds in full, and browser support.

## Build and test

From `crates/` inside the development shell:

```sh
cargo run -p drawbar
cargo test -p drawbar
```

`nix build .#drawbar-web` produces the whole servable bundle, the bound wasm
module beside `index.html`, in one step. The `--target wasm32-unknown-unknown`
builds must run from `crates/` or below, because `crates/.cargo/config.toml`
supplies the `--cfg=web_sys_unstable_apis` that WebUSB needs in `web-sys`.

## Disclaimer

Not affiliated with, authorized, or endorsed by Clavia DMI AB. "Nord", "Clavia",
and "Electro" are trademarks of Clavia DMI AB, used here only to identify the
hardware these formats come from.
