# 🎚️ drawbar
> #### ⚠️ Use at your own risk, this is alpha software ⚠️

Drawbar is a blackbox Clavia / Nord reverse engineering project in Rust that aims to be portable,
complete, and well-tested. The core of the project is [nord-format](crates/nord-format/README.md) --
a minimal dependency library that can read and write Nord keyboard files on Linux, macOS, Windows, and in the browser.
It is suitable to back any project that supports FFI with Rust (e.g. JS via wasm, Python via PyO3).

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

The latest released browser build is published at [jmoo.github.io/drawbar](https://jmoo.github.io/drawbar/),
with the user guide at [jmoo.github.io/drawbar/docs](https://jmoo.github.io/drawbar/docs/).
`nix build .#docs` builds that guide locally. As well as being a reference implementation, this repo
documents Nord file structure and protocols: the byte mapping tables are generated from the code and
browsable in the [rustdoc](https://jmoo.github.io/drawbar/docs/reference/file-formats.html).

## Status

This is still alpha software and should be used with caution. Drawbar is a blackbox reverse engineering
effort -- protocols and formats are decoded by interaction with real Nord devices rather than by
decompiling Clavia software, and hardware validation has focused on the **Electro 5**. Which instruments,
formats and USB operations are supported, and which claims are confirmed on hardware rather than inferred
from specimens, is listed under
[Supported instruments and formats](https://jmoo.github.io/drawbar/docs/getting-started/support.html).

## Disclaimer

Not affiliated with, authorized, or endorsed by Clavia DMI AB. (https://www.nordkeyboards.com)
"Nord", "Clavia", and "Electro" are trademarks of Clavia DMI AB, used here only to identify the
hardware these formats come from. Committed fixtures are self-generated files and
protocol captures. Proprietary sound libraries and firmware are not distributed here.
