# File formats

`nord-format` reads and writes Nord files. It is a pure library: no USB, no I/O
beyond `Read`, `Seek` and `Write`, and it builds for the browser.

## Round trips

Whatever the library reads, it writes back byte for byte. Regions it does not
decode are kept as raw bytes, and decoded values are views over them, so a file
survives a round trip even where its meaning is not fully known. Every fixture
and every file in the private corpus is checked this way, and every editable
field is set and read back to prove it moves no other bit.

## Large libraries

A piano library or sample instrument can run to hundreds of megabytes, and a
whole read holds all of it in memory. `npno::Index` and `nsmp::Index` read only a
file's headers and stroke directory, through `Read` and `Seek`, and give the byte
range of each stroke's audio. A caller reads one stroke by its range and decodes
it. The index does not verify the container checksum, which covers every byte;
`cbin::inspect` checks it in one streaming pass, and `cbin::Verifier` does the
same over chunks a caller supplies. Both report the body's CRC-32, the number an
instrument reports for the slot holding it, whichever checksum the file stores. `Verifier::seal` gives the checksum such
chunks call for, for a writer that streams a body before its checksum is known.

An edit is saved without a whole read too. A sample instrument's
`nsmp::Outline`, from its index, answers what a whole read answers outside the
audio and takes the same edits; `Index::patch` turns it into a `cbin::Patch`, the
few sections the edit changed and the restated checksum, which `Patch::copy`
writes while copying the file through. An edited piano library is written by
`Library::write_from`, which reads one stroke's audio at a time from the source
file; `npno::Index::still_matches` first says whether the file still holds the
directory the index read.

## Converting between generations

`nord_format::convert` moves a sample instrument between v2, v3 and v4 through one
model whose audio stays as the stream stores it: each stroke's fields on the
35,002 Hz lattice, its quantizer shift, and the landmarks its stream marks. A
conversion lays those fields out again in the target's units, so it never
resamples, and where the target's shift rule is coarser the result is the
stream the Sample Editor renders in that generation. `convert::plan` reads the
source and reports, before anything is written, what the target drops, holds
differently or fills in by rule, naming fields as the library does and unread
bytes by section and range. A loss the target can meet more than one way is a
choice the caller makes, and `Plan::apply` refuses while one is open.

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
- **Container-verified**: recognized, checksummed and kept byte for byte. Every
  other CBIN tag, the Lead SysEx banks, the `.cn3` library, and ZIP backup
  bundles behind the `bundle` feature.

Nord Sound Manager's bundles are a stored ZIP and a `meta.xml` manifest.
`nord_format::bundle` reads and writes both a member at a time, never holding the
archive, and refuses any archive laid out otherwise, so a bundle it reads writes
back byte for byte. It needs no feature.

Text notes are outside these tiers, because `nord-format` does not read them.
drawbar treats a file it cannot decode as a note when its bytes are UTF-8 text
of up to 256 KiB. In a note, Tab types a tab, and pasted control characters
other than tab and line breaks are dropped.
A file that begins with the magic of a format `nord-format` decodes, or whose
extension names one, is never a note, so one that fails to decode keeps its
error. An empty file is a note only when it is a `.txt`. See
[Editing](../drawbar/editing.md#notes).

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

`bundle` reads the members of ZIP archives as entities, backups and Drum banks
among them, and is off by default. `corpus` is for tests
only and points the sweep at the private corpus. See [Testing](testing.md).

To build the API documentation locally:

```sh
cd crates && nix develop -c cargo doc --no-deps --open
```
