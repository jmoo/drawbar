# 🎚️ drawbar
> # ⚠️ Use at your own risk, this is alpha software ⚠️

Drawbar is a blackbox Clavia / Nord reverse engineering project in rust that aims to be portable,
complete, and well-tested. The core of the project is [nord-format](crates/nord-format/README.md) -- 
a minimal dependency library can read and write nord keyboard files on linux, macos, windows, and in the browser.
It is suitable to back any project that that supports FFI with rust (ex. JS via wasm, Python via PyO3)

As well as being a reference implementation, this repo also serves as documentation of nord file structure
and protocols. Byte mapping tables are generated from code and can be browsed via rustdoc (`cargo doc --open`). 

## In this repo

| | Name | Description |
|---|------|-------------|
| 🎹 | [nord-format](crates/nord-format/README.md) | Clavia / Nord file parser/writer implementation in rust |
| 🧬 | [nord-bits-derive](crates/nord-bits-derive/README.md) | Declarative bit-packed panel definitions — the proc-macro behind nord-format |
| 🛠️ | [nord-cli](crates/nord-cli/README.md) | Command-line tool for interacting with Clavia / Nord keyboards and files |
| 🔌 | [nord-usb](crates/nord-usb/README.md) | Clavia / Nord USB protocol implementation in rust |
| 🎚️ | [drawbar](crates/drawbar/README.md) | Cross platform gui app for Clavia / Nord keyboards — view, edit, transfers, and more —  for windows, macos, linux, and web |

## Status

This is still alpha software and should be used with caution. Drawbar is a blackbox reverse engineering effort -- 
it does not lean on decompilation of Clavia software. Instead, protocols and formats are decoded by interaction 
with real nord devices.

Drawbar began as a project to reverse engineer the Nord Electro 5. It can now can recognize most Clavia file types
and read/write most stage models as well (Stage 2, 3, and 4) thanks to documentation created by other community
reverse engineering efforts. It can also read and write most nord sample files, although the audio codec is not currently
understood. The USB protocol is also mostly understood and implemented, although only tested on one device.

Only the Electro 5 has had thorough on device testing and validation, but all codecs are tested round-trip 
against a corpus of >10,000 real nord file specimen found in the wild. This includes files from most Clavia products and 
captured replays of usb communication.

## Disclaimer

Not affiliated with, authorized, or endorsed by Clavia DMI AB. (https://www.nordkeyboards.com) 
"Nord", "Clavia", and "Electro" are trademarks of Clavia DMI AB, used here only to identify the
hardware these formats come from. All Clavia / Nord artifacts included in this repo
are synthetic test artifacts produced by the author of this repo.