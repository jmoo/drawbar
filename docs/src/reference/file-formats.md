# File formats

`nord-format` reads and writes Nord files. It is a pure library: no USB, no I/O
beyond `Read`, `Seek` and `Write`, and it builds for the browser. Its
[crate documentation](https://docs.rs/nord-format/latest/nord_format/) on docs.rs is the reference for the API, the
formats and their bytes.

## Round trips

Whatever the library reads, it writes back byte for byte. Regions it does not
decode are kept as raw bytes, and decoded values are views over them, so a file
survives a round trip even where its meaning is not fully known.
[Testing](testing.md) says how this is checked.

## The support map

The [`formats`](https://docs.rs/nord-format/latest/nord_format/formats/) module lists every format and how far its
decoding goes, and each format's own documentation says what a read validates,
where its field placements came from, and its byte map. There are three tiers:

- **Decoded**: every field named.
- **Structurally decoded**: the container and layout are understood, and the
  audio is decoded and encoded. These are sample instruments, piano libraries in
  both formats (5.x and 6.x), and Sample Editor projects.
- **Container-verified**: recognized, checksummed and kept byte for byte.
  Everything else.

The `formats` documentation names the formats in each tier, and
[What is supported](../getting-started/support.md) says what drawbar does with
each instrument's files.

Both generations of the [`cbin`](https://docs.rs/nord-format/latest/nord_format/cbin/) container are read and written: the
current one with a CRC-32 over the body, and the older one with a CRC-16 over the
whole file.

Nord Sound Manager's bundles are a stored ZIP and a `meta.xml` manifest.
[`bundle`](https://docs.rs/nord-format/latest/nord_format/bundle/) reads and writes both a member at a time, never holding
the archive, and refuses any archive laid out otherwise, so a bundle it reads
writes back byte for byte. It needs no feature. Reading an archive as an
`Entity`, through `from_path` or `from_stream`, needs the `bundle` feature, and
such an entity is read-only. The [features](https://docs.rs/nord-format/latest/nord_format/#features-and-dependencies) are
listed on docs.rs.

## Large libraries

A piano library or sample instrument can run to hundreds of megabytes, so none
of these needs a whole read:

- [`npno::Index`](https://docs.rs/nord-format/latest/nord_format/formats/npno/struct.Index.html) and
  [`nsmp::Index`](https://docs.rs/nord-format/latest/nord_format/formats/nsmp/struct.Index.html) read a file's headers and
  stroke directory and give the byte range of each stroke's audio.
- [`cbin::inspect`](https://docs.rs/nord-format/latest/nord_format/cbin/fn.inspect.html) and
  [`cbin::Verifier`](https://docs.rs/nord-format/latest/nord_format/cbin/struct.Verifier.html) check the container
  checksum in one streaming pass.
- A sample instrument's [`nsmp::Outline`](https://docs.rs/nord-format/latest/nord_format/formats/nsmp/struct.Outline.html)
  takes the same edits as a whole read and saves them as a
  [`cbin::Patch`](https://docs.rs/nord-format/latest/nord_format/cbin/struct.Patch.html) applied while the file is copied.
- An edited piano library is written by
  [`Library::write_from`](https://docs.rs/nord-format/latest/nord_format/formats/npno/struct.Library.html#method.write_from),
  one stroke's audio at a time from the source file.

## The field registry

Every decoded layout generates a registry. `fields()` lists each field with its
path, placement, current value, accepted values and control kind, and
`set_field(path, value)` writes one back through the type's own parser. That is
what `nord edit --fields` prints and what drawbar's Advanced table shows, so a
field becomes editable by being declared. [`panel::of`](https://docs.rs/nord-format/latest/nord_format/panel/fn.of.html)
adds the layout for formats that have one: which controls sit together, in what
order, and which the instrument is using for the state the file holds.

## Text notes

drawbar's notes are outside these tiers, because `nord-format` does not read
them. drawbar treats a file it cannot decode as a note when its bytes are UTF-8
text. A file that begins with the magic of a format `nord-format` decodes, or
whose extension names one, is never a note, so one that fails to decode keeps
its error. An empty file is a note only when it is a `.txt`. What a user sees is
under [Notes](../drawbar/editing.md#notes).

To build the API documentation locally:

```sh
cd crates && nix develop -c cargo doc --no-deps --open
```
