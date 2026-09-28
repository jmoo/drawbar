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
- `demo/`: instruments small enough for drawbar to ship as a demo: a looped pad
  as v2 and v4 sample instruments from `nord sample encode`, and a three-root tine
  piano library from `nord piano build`, all from synthesized WAVs.
- `nsmp/`: a mono triangle wave encoded by `nsmp::encode` as a v3 sample
  instrument (`.nsmp3`), the generation no other fixture covers.
- `npno/`: the triangle wave as a piano library from `npno::encode::build`.
- `zip/`: stored archives, one for each way the reader classifies a ZIP: an
  Electro 5 bundle holding a program, a song, and the triangle wave as a v2
  sample and as a piano library from `npno::encode::build`; a Drum 2 bank; a
  Drum 3 bank; and a plain bundle of CBIN members. The members other than the
  wave are fixtures from `ne5/` and `cbin/`. They need the `bundle` feature.
- `sysex/` and `midi/`: the MIDI 1.0 Universal Identity Request, a public
  standard message naming no manufacturer, as a SysEx dump and as a one-track
  Standard MIDI File.
- `cn3/`: the `CNE3` magic, zero-padded to the 12 bytes the sniffer reads.
  Nothing else of that format is known.

`cargo test -p nord-format --features bundle --test generate_fixtures --
--ignored` writes the last six directories again.

The sweep checks their checksums, decoding, exact round trips, field isolation,
and oracle sidecars. It fails when this tree lacks a file of a type the reader
dispatches: each container class the sweep reads and each tag in
`nord_format::cbin_formats`. A corpus under `NORD_CORPUS_ROOT` has no such
requirement; any tree of Nord files is one.
