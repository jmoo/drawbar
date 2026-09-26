# Synthetic fixtures

Specimen files written by this crate's own writers, with no instrument bytes or
vendor material. They give the sweep in `tests/corpus` a tree to read in any
checkout, without the corpus. They follow the corpus conventions: any file the
reader recognizes is a specimen, and a `<file>.oracle.json` beside it states
what was set.

- `cbin/<tag>.g<N>.cbin`: each CBIN tag in `tests/support/format_table.rs`, in
  both header generations, with a zero-filled body. The container is the
  specimen.
- `ne5/`: Electro 5 files built through the public constructors and setters:
  the defaults, programs with one panel edit each, a settings edit, and a song.
  A sidecar pins each edit.
- `nsmpproj/`: Sample Editor projects from `nsmpproj::Project::new`: one zone,
  three zones, and three zones with the middle one retuned and its range moved.
  A sidecar pins each one.

The corpus sweep checks their checksums, decoding, exact round trips, field
isolation, and oracle sidecars.
