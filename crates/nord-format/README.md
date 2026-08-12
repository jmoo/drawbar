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
| `ne5s` settings | ✅ | ✅ | System / MIDI / Sound menus plus the `startup_*` state the instrument restores at power-up ✅ — 32 of the 34 cataloged settings are pinned by a change-one-setting hardware sweep; memory protect and local control move no bit of the body and stay undecoded. |
| `nsmp` sample | ✅ | ✅ | Name, categories, keyboard zones and per-zone strokes (root key, top note) — the audio stays encoded and is carried verbatim, so instruments can be renamed, retuned and remapped but not synthesised. The nsmp3/nsmp4 generations share the tag; they parse and round-trip with the body verbatim (the version word says which schema a file holds). |
| `npno` piano | ✅ (container) | ✅ | Body unmapped; carried verbatim, so the file round-trips byte-exact and the checksum is verified. |
| `ns3f`/`ns3l` Stage 3 program | ✅ | ✅ | **Every documented parameter placed, on both panels** — 588 fields. The byte maps document one panel; a program holds two — Panel A and Panel B, the instrument's two independent setups — of the same layout 263 bytes apart, found from the corpus and enforced by a test. The panel is one `#[bitbody]` placed twice, so `panel_a.organ_type` and `panel_b.organ_type` are the same declaration. Values are named where the maps enumerate them and raw where they defer to the viewer's sources. Not confirmed on hardware. |
| `ns3y` Stage 3 synth preset | ✅ | ✅ | Panel A's synth block under its own tag — located in the corpus, not from the docs, whose stated offset does not reproduce. |
| `ns2p`/`ns2l` Stage 2 program | ✅ | ✅ | The same, over the Stage 2's map and its two 249-byte slots: 838 fields. |
| `ns4p`/`ns4l` Stage 4 program | ✅ | ✅ | **Every parameter placed** — 878 of them, all four engines and the globals, covering 76% of the body's bits. Values are raw: the number the file stores, range-checked to its slot, with nothing interpreting it into a panel name yet. Offsets derived from ns4decode's published tables (MIT); not confirmed on hardware. |
| `ns4y`/`ns4n`/`ns4o` Stage 4 presets | ✅ | ✅ | The synth, piano and organ preset banks: one program section each, same placements and the same raw values. |
| every other corpus format | ✅ (container) | ✅ | Container-verified stubs, one module per tag with its observed body length: Electro 3/4/6/7, Piano 1–5, Grand, Stage Classic/EX, Stage 2/3 satellites (songs, synths, settings) and the Stage 4 settings, Wave 1/2, C2/C2D, `no3`, Lead 4/A1, Drum 2/3P — 60+ CBIN tags in all. |
| Lead 1/2/2X/3 SysEx and MIDI banks | ✅ | ✅ | Carried verbatim; the two SysEx envelope families (`F0 33 · 04` vs `F0 33 · 09`) classify the model line. |
| `.cn3` Electro 2 library | ✅ | ✅ | `CNE3` magic, not CBIN; verbatim. |
| backup bundle (ZIP) | ✅ | — | Partial; behind the `bundle` feature. Drum 2 banks / Drum 3P kit banks walk to their CBIN members. |

Everything that parses **round-trips byte-for-byte**, verified against a
change-one-knob specimen corpus. **Both `CBIN` container generations are read and
written** — type-1 (crc32 over the body) and the older type-0 (trailing crc16 over
the whole file), so factory files round-trip too — and `inspect` reports container
facts (tag, generation, version, length, checksum verdict) for *any* CBIN file in
O(1) memory, mapped body or not.

## Usage

```rust
use nord_format::{from_path, Entity, Program};
use nord_format::bank::Item; // for `.location()`
use nord_format::formats::ne5::OrganModel;

let entity = from_path("patch.ne5p")?;

if let Entity::Program(Program::Electro5(p)) = entity {
    // `p` is a `Cbin<ne5::Program>`: the CBIN header plus the decoded body,
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

`from_path` / `from_stream` sniff the container and return an [`Entity`]. Each
format lives under `formats::`, named for the four-character CBIN tag it carries —
or, where a model family shares a prefix across several tags, for that prefix. So
`formats::ne5` holds the concrete `Program`/`Song`/`Settings` layouts and the
`OrganModel` / `VibChorus` / `PercSpeed` decode types, while `formats::nsmp` and
`formats::npno` are single formats shared across the line.

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
`cargo test`, as does `tests/dispatch.rs`, which synthesizes a file for every
registered tag and checks dispatch + round-trip for both header generations.
The **corpus integration suites** (`tests/ne5.rs` for Electro 5 depth,
`tests/corpus.rs` for the all-model sweep) are gated behind the `corpus` feature
because they need the specimen corpus, which lives in a separate private repo
(`jmoo/nord-corpus`)

```sh
cargo test -p nord-format                       # minimal suite (unit + dispatch)

# Full corpus sweep — point at a nord-corpus checkout:
NORD_CORPUS_ROOT=/path/to/nord-corpus \
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

Every other model in the corpus now parses and round-trips at the container
level — 10,000+ specimens across 70 formats in the sweep — with typed stubs
waiting for their bodies to be mapped. The Stage 2/3 programs decode their
program-wide globals, and the Stage 4's program and preset bodies have every
parameter placed, though nothing yet interprets the values. Deeper decode work
happens per model from here; the Electro 5 remains the only body whose values
are named as well as placed.

Expect refactoring, API changes, and other misc changes until a stable version is released.

## Prior art

 Shout out to @Chris55. I wish I had discovered his work before I started. It would have made some of the initial reverse engineering easier. Once I finish decoding the ne5, I plan to contribute the ICD back to these projects.

- **[`Chris55/ns3-program-viewer`](https://github.com/Chris55/ns3-program-viewer)**
  — a read-only web viewer for Nord Stage 2 / 2EX / 3 programs.
- **[`Chris55/nord-documentation`](https://github.com/Chris55/nord-documentation)**
  ([rendered](https://chris55.github.io/nord-documentation/)) — community byte-map
  docs for Nord Stage 2/3 and Lead A1, built with the same hex-diff method.
- **[`ns4decode`](https://ns4decode.netlify.app)** (MIT, © 2024 Randy) — a Stage 4
  program and preset viewer in Python, which publishes complete offset tables for
  the `ns4p` body. The Stage 4 placements here are derived from those tables; the
  value tables it also publishes are not used.

Why maintain a separate project? Primarily because this is a rust learning project for me and I've written enough js in my life to not really want to write it in my free time. Also, the goal of this project are r/w + usb, not just read.

## Disclaimer

Not affiliated with, authorized, or endorsed by Clavia DMI AB. "Nord", "Clavia",
and "Electro" are trademarks of Clavia DMI AB, used here only to identify the
hardware these formats come from. All reverse engineering is of files produced by
Nord hardware, for interoperability.

[`crcxx`]: https://docs.rs/crcxx
[`Entity`]: https://docs.rs/nord-format
