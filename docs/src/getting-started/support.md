# Supported instruments and formats

Drawbar began with the **Electro 5**. Hardware validation has focused on it, and
specimen evidence is not hardware evidence, so each row below keeps the qualifier
its claim was established with.

| Family | State |
|---|---|
| Electro 5 program, live, song, settings | Decoded and fully supported; values pinned by hardware sweeps |
| Stage 2, 3 and 4 programs and selected presets | Supported from community documentation and specimen evidence; **not validated on hardware** |
| Sample instruments (`nsmp`, `nsmp3`, `nsmp4`) | Encoding and decoding of v2/v3/v4; only v2 confirmed with hardware playback |
| Piano libraries (`npno`) | Rename, retune, remap, trim and split — confirmed on hardware for a dropped bank and dropped velocity layers; renames, retunes, remaps and a narrowed key range are inferred from specimens and have not been played |
| Everything else | CBIN tags across the model line, the Lead SysEx/MIDI banks and the `.cn3` Electro 2 library — recognized and carried verbatim without decoding their parameters |

Writable entities round-trip byte-for-byte, verified against a change-one-knob
specimen corpus. That establishes preservation of file bytes. It does not
establish that every decoded parameter or newly encoded sound behaves correctly
on hardware.

The support map itself lives in the `nord-format` rustdoc, beside the code it
describes, and defines the three tiers — decoded, structurally decoded,
container-verified stub — with each decoded body's byte map:

- [The `formats` module on docs.rs](https://docs.rs/nord-format/latest/nord_format/formats/)
- [File formats](../reference/file-formats.md) in this guide

Management of Nord devices over USB is supported. Which operations are
implemented, and which of them are hardware-verified, is listed under
[USB protocol](../reference/usb-protocol.md).
