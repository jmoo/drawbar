# nord-format

Parse and write **Clavia / Nord** keyboard binary file formats from Rust.

This is the pure format-logic crate of the drawbar toolkit: the `CBIN` container
(both header generations, each with its checksum), and per-model entity layouts
declared once with `#[bitbody]`. Its dependencies are [`crcxx`], [`thiserror`],
and the matching `nord-bits-derive` crate (plus `zip` behind the `bundle` feature
for backup bundles). It does no USB, OS, or I/O beyond `Read`/`Seek`/`Write` — so
it's trivially testable against a specimen corpus, reusable by higher layers (a
device/USB crate, a CLI) without dragging in a transport stack, and portable
anywhere `std` is, wasm included.

**Lossless round-trip is the core invariant.** Unknown regions are kept as raw
byte blocks and decoded values are exposed as read-only views over them, so
`parse → write` is byte-identical even where the semantics are incomplete — and a
newly decoded field is never a risk to the write path.

## Usage

`from_path` / `from_stream` sniff the container and return an [`Entity`]; each
format lives under `formats::`, named for the four-character CBIN tag it carries.

```rust
use nord_format::{from_path, Entity, Program};
use nord_format::bank::Item; // for `.location()`
use nord_format::formats::ne5::OrganModel;

let entity = from_path("patch.ne5p")?;

if let Entity::Program(Program::Electro5(p)) = entity {
    // `p` is a `Cbin<ne5::Program>`: the CBIN header plus the decoded body,
    // and it derefs to the body, so the panels read as fields.
    println!("location: {:?}", p.location());
    println!("gain: {:?}", p.center_panel.gain);

    // Organ state is decoded per model + selected preset:
    let preset = p.organ_panel.preset(OrganModel::B3);
    println!("B3 drawbars: {:?}", p.organ_panel.drawbars(OrganModel::B3, preset));
}
```

Every `#[bitbody]` also generates a field registry, so a field is settable by
being declared — no table of names on the consumer's side:

```rust
use nord_format::{from_path, to_bytes};

let mut entity = from_path("patch.ne5p")?;
if let Some(registry) = entity.registry_mut() {
    registry.set_field("center_panel.gain", "96")?;
}
std::fs::write("out.ne5p", to_bytes(&entity)?)?;
```

## Build & test

From `crates/` in the development shell:

```sh
cargo test -p nord-format          # unit + dispatch + the committed fixtures sweep
```

`bundle` adds ZIP-based backup bundles and is off by default so parse-only
consumers stay lean. The test-only `corpus` feature adds the private specimen
corpus at `NORD_CORPUS_ROOT` to the sweep (and implies `bundle`, because the
corpus holds ZIP banks): `nix build .#nord.nord-format-corpus`. `nix build
.#nord-format` builds and tests the crate on its own.

## More

- [`nord-format` on docs.rs](https://docs.rs/nord-format) — the support map lives
  in the [`formats`] module docs, beside the code it describes
- [File formats][guide] — the support tiers, the round-trip invariant, and the
  field registry
- [Testing][testing] — what each suite proves

## Disclaimer

Not affiliated with, authorized, or endorsed by Clavia DMI AB. "Nord", "Clavia",
and "Electro" are trademarks of Clavia DMI AB, used here only to identify the
hardware these formats come from. All reverse engineering is of files produced by
Nord hardware, for interoperability.

[`crcxx`]: https://docs.rs/crcxx
[`thiserror`]: https://docs.rs/thiserror
[`Entity`]: https://docs.rs/nord-format
[`formats`]: https://docs.rs/nord-format/latest/nord_format/formats/
[guide]: https://jmoo.github.io/drawbar/docs/reference/file-formats.html
[testing]: https://jmoo.github.io/drawbar/docs/reference/testing.html
