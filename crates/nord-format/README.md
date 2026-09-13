# nord-format

**Read and write Nord keyboard files from Rust, byte for byte.**

Programs, live slots, set lists, settings, sample instruments and piano
libraries, from a library with no I/O of its own beyond `Read`, `Seek` and
`Write`. It builds anywhere `std` does, including the browser.

What you read, you can write back unchanged. Anything the library does not yet
decode is kept as raw bytes, so a file survives a round trip even where its
meaning is not fully known, and every editable field is checked to move no other
bit.

## Usage

`from_path` and `from_stream` detect the format and return an `Entity`. Decoded
fields read as struct fields:

```rust
use nord_format::{from_path, Entity, Program};
use nord_format::bank::Item; // for `.location()`
use nord_format::formats::ne5::OrganModel;

let entity = from_path("patch.ne5p")?;

if let Entity::Program(Program::Electro5(p)) = entity {
    // `p` is the CBIN header plus the decoded body, and derefs to the body.
    println!("location: {:?}", p.location());
    println!("gain: {:?}", p.center_panel.gain);

    let preset = p.organ_panel.preset(OrganModel::B3);
    println!("B3 drawbars: {:?}", p.organ_panel.drawbars(OrganModel::B3, preset));
}
```

Every layout also has a field registry, so any field can be set by its path:

```rust
use nord_format::{from_path, to_bytes};

let mut entity = from_path("patch.ne5p")?;
if let Some(registry) = entity.registry_mut() {
    registry.set_field("center_panel.gain", "96")?;
}
std::fs::write("out.ne5p", to_bytes(&entity)?)?;
```

## Learn more

- The [`formats` module](https://docs.rs/nord-format/latest/nord_format/formats/)
  on docs.rs lists every supported file and how far its support goes.
- [File formats](../../docs/src/reference/file-formats.md) in the guide explains
  the support tiers and the registry.
- [Testing](../../docs/src/reference/testing.md) explains what the fixture sweep
  and the private corpus prove.

## Building

`cargo test -p nord-format` from `crates/` in `nix develop`. The `bundle` feature
adds ZIP backup bundles. The test-only `corpus` feature points the sweep at the
private corpus.

## Disclaimer

Not affiliated with, authorized, or endorsed by Clavia DMI AB. "Nord", "Clavia",
and "Electro" are trademarks of Clavia DMI AB, used here only to identify the
hardware these formats come from. All reverse engineering is of files produced by
Nord hardware, for interoperability.
