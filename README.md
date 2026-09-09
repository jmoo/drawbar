# 🎚️ drawbar
> #### ⚠️ Use at your own risk, this is alpha software ⚠️

Drawbar is a blackbox Clavia / Nord reverse engineering project in Rust that aims to be portable,
complete, and well-tested. The core of the project is [nord-format](crates/nord-format/README.md) --
a minimal dependency library that can read and write Nord keyboard files on Linux, macOS, Windows, and in the browser.
It is suitable to back any project that supports FFI with Rust (e.g. JS via wasm, Python via PyO3).

As well as being a reference implementation, this repo also serves as documentation of Nord file structure
and protocols. Byte mapping tables are generated from code and can be browsed via rustdoc
(`cd crates && nix develop -c cargo doc --no-deps --open`).

## In this repo

| | Name | Description |
|---|------|-------------|
| 🎹 | [nord-format](crates/nord-format/README.md) | Clavia / Nord file parser/writer implementation in Rust |
| 🧬 | [nord-bits-derive](crates/nord-bits-derive/README.md) | Declarative bit-packed panel definitions — the proc-macro behind nord-format |
| 🛠️ | [nord-cli](crates/nord-cli/README.md) | Command-line tool for interacting with Clavia / Nord keyboards and files |
| 🔌 | [nord-usb](crates/nord-usb/README.md) | Clavia / Nord USB protocol implementation in Rust |
| 🎚️ | [drawbar](crates/drawbar/README.md) | Cross-platform GUI app for Clavia / Nord keyboards — view, edit, transfers, and more — for Windows, macOS, Linux, and web |

## Try it out!

```
# Run nord-cli
nix run .#nord-cli -- program get 1:1

# Run the desktop app
nix run .#drawbar

# Run drawbar in the browser
nix run .#drawbar-web
```

## Status

This is still alpha software and should be used with caution. Drawbar is a blackbox reverse engineering effort --
it does not lean on decompilation of Clavia software. Instead, protocols and formats are decoded by interaction
with real Nord devices.

Drawbar began with the Nord **Electro 5**. Its **program**, **live**, **song**, and **settings** layouts are decoded and fully supported.
**Stage 2**, **3**, and **4** programs and selected presets are supported thanks to community documentation and specimen evidence
but have not been validated on hardware. **Sample** support includes encoding and decoding of v2/v3/v4 instruments (only v2 confirmed with hardware playback).
**Pianos** can be renamed, retuned, remapped, trimmed and split, confirmed on hardware.

Management of Nord devices over USB is supported. The [USB status](crates/nord-usb/README.md#status) lists implemented 
operations and their hardware validation.

Many other formats are recognized and preserved verbatim without decoding their parameters;
see the [format support tiers](crates/nord-format/README.md#what-it-handles).

Hardware validation has focused on the Electro 5. Public tests use self-generated fixtures and
USB replay scripts; optional private corpus tests check byte-exact file round trips, field isolation,
and captured protocol exchanges. Round-trip tests establish preservation of file bytes;
they do not establish that every decoded parameter or newly encoded sound behaves correctly on hardware.

## Disclaimer

Not affiliated with, authorized, or endorsed by Clavia DMI AB. (https://www.nordkeyboards.com)
"Nord", "Clavia", and "Electro" are trademarks of Clavia DMI AB, used here only to identify the
hardware these formats come from. Committed fixtures are self-generated files and
protocol captures. Proprietary sound libraries and firmware are not distributed here.
