# File formats

`nord-format` reads and writes Nord files. It is a pure library: no USB, no I/O
beyond `Read`, `Seek` and `Write`, and it builds for the browser.

## Round trips

Whatever the library reads, it writes back byte for byte. Regions it does not
decode are kept as raw bytes, and decoded values are views over them, so a file
survives a round trip even where its meaning is not fully known. Every fixture
and every file in the private corpus is checked this way, and every editable
field is set and read back to prove it moves no other bit.

## The support map

The authoritative list is in the `formats` module documentation on
[docs.rs](https://docs.rs/nord-format/latest/nord_format/formats/), beside the
code. Each decoded body's documentation says how far reading and writing go, what
a read validates, where the field placements came from, and shows the generated
byte map. There are three tiers:

- **Decoded**: every field named. Electro 5 programs, live slots, set lists and
  settings; Stage 2, 3 and 4 programs and live slots; the Stage 3 synth preset;
  the Stage 4 synth, piano and organ presets.
- **Structurally decoded**: the container and layout are understood, and the
  audio is decoded and encoded. Sample instruments in every layout, piano
  libraries, and Sample Editor projects.
- **Container-verified**: recognised, checksummed and carried verbatim. Every
  other CBIN tag, the Lead SysEx banks, the `.cn3` library, and ZIP backup
  bundles behind the `bundle` feature.

Text files are none of these tiers. `nord-format` does not read them at all: it
is drawbar that calls a file it cannot decode a note when the bytes are words,
and edits it as the text it is. A file that begins with the magic of a format
`nord-format` decodes is never a note, so one that fails to decode keeps its
error. See [Editing](../drawbar/editing.md).

Both generations of the `CBIN` container are read and written: the current one
with a CRC-32 over the body, and the older one with a CRC-16 over the whole file.

## The field registry

Every `#[bitbody]` layout generates a registry. `fields()` lists each field with
its path, placement, current value and accepted values, and `set_field(path,
value)` writes one back through the type's own parser. That is what `nord edit
--fields` prints and what drawbar's Advanced table shows, so a field becomes
editable by being declared.

Each field carries its control kind: a knob with a unit, a bipolar knob, a
selector, a drawbar, a morph slot, a pattern grid, or a library reference.
`panel::of` adds the layout for formats that have one: which controls sit
together, in what order, and which the instrument is using for the state the file
holds.

```rust
let entity = nord_format::from_path("patch.ne5p")?;
if let Some(panel) = nord_format::panel::of(&entity) {
    // ordered, nestable groups, each with a condition over the body's values
}
```

## Features

`bundle` enables ZIP backup bundles and is off by default. `corpus` is for tests
only and points the sweep at the private corpus. See [Testing](testing.md).

To build the API documentation locally:

```sh
cd crates && nix develop -c cargo doc --no-deps --open
```
