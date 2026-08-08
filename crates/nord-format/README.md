# nord-format

Parse and write **Clavia / Nord** keyboard binary file formats from Rust.

This is the pure format-logic crate of the Nord toolkit: the `CBIN` container
(both header generations, each with its checksum), and per-model entity layouts
declared once with `#[bitbody]`. It depends only on [`crcxx`] (plus `zip` behind
the `bundle` feature for backup bundles) and does no USB, OS, or I/O beyond
`Read`/`Seek`/`Write` — so it's trivially testable against a specimen corpus and
reusable by higher layers (a device/USB crate, a CLI) without dragging in a
transport stack.

## What it handles

| Format | Parse | Write (byte-exact) | Semantic decode |
|---|:--:|:--:|---|
| `ne5p` program | ✅ | ✅ | Center / piano / sample / FX / EQ panels ✅. **Organ**: drawbars, presets, vibrato/chorus, B3 percussion, and the B3-bass / Farfisa forms ✅ — three organ-block bits remain unexplained. |
| `ne5t` song / set | ✅ | ✅ | ✅ (four program slots) |
| `ne5l` live slot | ✅ | ✅ | Same as `ne5p` — the live buffer is the program body under another tag (confirmed on hardware), so it shares the program body type in the three live slots. |
| `ne5s` settings | ✅ | ✅ | System / MIDI / Sound menus plus the `startup_*` state the instrument restores at power-up ✅ — 32 of the 34 catalogued settings are pinned by a change-one-setting hardware sweep; memory protect and local control move no bit of the body and stay undecoded. |
| `nsmp` sample | ✅ | ✅ | Name, categories, keyboard zones and per-zone strokes (root key, top note) — the audio stays encoded and is carried verbatim, so instruments can be renamed, retuned and remapped but not synthesised. |
| `npno` piano | ✅ (container) | ✅ | Body unmapped; carried verbatim, so the file round-trips byte-exact and the checksum is verified. |
| backup bundle (ZIP) | ✅ | — | Partial; behind the `bundle` feature. |

Everything that parses **round-trips byte-for-byte**, verified against a
change-one-knob specimen corpus. **Both `CBIN` container generations are read and
written** — type-1 (crc32 over the body) and the older type-0 (trailing crc16 over
the whole file), so factory files round-trip too — and `inspect` reports container
facts (tag, generation, version, length, checksum verdict) for *any* CBIN file in
O(1) memory, mapped body or not.

## Usage

```rust
use nord_format::{from_path, Entity, Program};
use nord_format::common::bank::Item; // for `.location()`
use nord_format::electro5::OrganModel;

let entity = from_path("patch.ne5p")?;

if let Entity::Program(Program::Electro5(p)) = entity {
    // `p` is a `Cbin<electro5::Program>`: the CBIN header plus the decoded body,
    // and it derefs to the body, so the panels read as fields.
    println!("location: {:?}", p.location());
    println!(
        "lower/upper: {:?} / {:?}",
        p.center_panel.lower_part, p.center_panel.upper_part
    );

    // Organ state is decoded per model + selected preset:
    let preset = p.organ_panel.preset(OrganModel::B3);
    println!("B3 drawbars: {:?}", p.organ_panel.drawbars(OrganModel::B3, preset));
    println!("B3 vibrato:  {:?}", p.organ_panel.vib_type(OrganModel::B3));
}
```

`from_path` / `from_stream` sniff the container and return an [`Entity`]; the
`electro5` module holds the concrete `Program`/`Song`/`Settings` layouts and the
`OrganModel` / `VibChorus` / `PercSpeed` decode types.

## Lossless round-trip is the core invariant

Unknown regions are kept as **raw byte blocks** and decoded values are exposed as
read-only views over them, so `parse → write` is byte-identical even where the
semantics are incomplete. Every newly decoded field is a safe, incremental
refinement — never a risk to the write path.

## Features

- **`bundle`** — ZIP-based backup bundles (pulls in the `zip` stack). Off by
  default so parse-only consumers stay lean; enable with `--features bundle`.
- **`corpus`** — *test-only*. Gates the corpus-backed integration tests
  (`tests/ne5.rs`); see below.

## Tests

Unit tests live inline (`#[cfg(test)] mod tests`) and run on a plain
`cargo test`. The **corpus integration suite** (`tests/ne5.rs`) is gated behind
the `corpus` feature because it needs the specimen corpus, which lives in a
separate private repo (`jmoo/nord-corpus`)

```sh
cargo test -p nord-format                       # minimal suite (inline unit tests)

# Full corpus sweep — point NORD_CORPUS_DIR at a nord-corpus/ne5 checkout:
NORD_CORPUS_DIR=/path/to/nord-corpus/ne5 \
  cargo test -p nord-format --features corpus

# With nix
nix build .#checks.<system>.nord-format-corpus
```

## Where this fits

This project started in 2023 as my personal 'learn rust' project. The goal was to be able to read/write clavia files for my electro 5. After I got most of the ne5 formats RE'd, I got a new job that was pretty demanding and I didn't have much free time to continue.

The goal now is to finish reverse engineering ne5 files, add support for more models, and to reverse engineer the USB protocol. The end game is to have a portable toolkit that could be the foundation of an open source nord manager alternative with linux support.

## Current State

Incomplete but solid for reading and writing electro 5 files due to lossless round trips and large test corpus.
Nord Electro 5 Program, Song, Live and Settings files are nearly 100% solved for the file versions I've encountered, and the library safely errors when
encountering unexpected files or versions. Sample instruments read, write and edit
(metadata only — the audio codec is not decoded); piano files round-trip verbatim but
their body is unmapped; bundles and backups are still incomplete.

Expect refactoring, API changes, and other misc changes until a stable version is released. I would also like to support
more nord models, but will not work on those until I'm 100% satisfied with the Electro 5 support.

## Prior art

 Shout out to @Chris55. I wish I had discovered his work before I started. It would have made some of the initial reverse engineering easier. Once I finish decoding the ne5, I plan to contribute the ICD back to these projects.

- **[`Chris55/ns3-program-viewer`](https://github.com/Chris55/ns3-program-viewer)**
  — a read-only web viewer for Nord Stage 2 / 2EX / 3 programs.
- **[`Chris55/nord-documentation`](https://github.com/Chris55/nord-documentation)**
  ([rendered](https://chris55.github.io/nord-documentation/)) — community byte-map
  docs for Nord Stage 2/3 and Lead A1, built with the same hex-diff method.

Why maintain a separate project? Primarily because this is a rust learning project for me and I've written enough js in my life to not really want to write it in my free time. Also, the goal of this project are r/w + usb, not just read.

## Disclaimer

Not affiliated with, authorized, or endorsed by Clavia DMI AB. "Nord", "Clavia",
and "Electro" are trademarks of Clavia DMI AB, used here only to identify the
hardware these formats come from. All reverse engineering is of files produced by
hardware the author owns, for interoperability.

[`crcxx`]: https://docs.rs/crcxx
[`Entity`]: https://docs.rs/nord-format
