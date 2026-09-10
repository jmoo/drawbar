# nord-usb

Talk to **Clavia / Nord** keyboards over USB from Rust — the vendor protocol Nord
Sound Manager speaks, reverse-engineered from packet captures.

This is the transport-and-protocol half of the Nord toolkit.
[`nord-format`](../nord-format) owns the bytes of a file; this crate owns getting
those bytes on and off the instrument. It depends on `nord-format` for the
container it wraps read data in, and on nothing else at its core — the backends
are optional features: `nusb` for the desktop (macOS, Linux, Windows; pure Rust),
`web` for the browser over WebUSB, and `replay` to drive the protocol from
committed captures with no hardware at all.

> [!WARNING]
> Alpha software driving real hardware over a reverse-engineered protocol. The
> verbs listed under [USB protocol][guide] are hardware-verified; everything else
> is not. Back up your instrument (Nord Sound Manager makes a full backup) before
> pointing anything here at sounds you can't re-create.

## Usage

```rust
use nord_usb::{op, Location, ObjectClass, Session};
use nord_usb::transport::UsbTransport;

let mut transport = UsbTransport::open_first()?;

// Read-only by default — the type system will not let a mutating op through.
// `from_user` takes the instrument's own one-indexed numbering: 7:4 on the panel.
let mut session = Session::open(&mut transport, ObjectClass::Program).await?;
let at = Location::from_user(7, 4);
let info = op::info(&mut session, at).await?;
let file = op::read_program(&mut session, at).await?;
session.commit().await?;
```

Mutating operations need the capability to be asked for explicitly
(`session.allow_destructive_writes()`).

**Always `commit()`, including on the error path.** The closing exchanges are what
clear the instrument's progress display; abandoning a transaction after a progress
label has been sent leaves the device stuck until it is power-cycled. `Session`
carries a `Drop` assertion to catch the mistake in debug builds.

## Build & test

From `crates/` in the development shell:

```sh
cargo test -p nord-usb --features replay
```

⚠️ `replay` is not a default feature, so a bare `cargo test -p nord-usb` compiles
the replay tests out and reports a pass having verified none of the wire encoding.
The Nix build enables it via `[package.metadata.nix] testFeatures` in `Cargo.toml`
(`nix build .#nord-usb`); `--features corpus` adds the captures in the private
corpus at `NORD_CORPUS_ROOT` and implies `replay`. A `web` build needs
`--cfg=web_sys_unstable_apis`, which `crates/.cargo/config.toml` supplies for the
wasm target, so run it from `crates/` or below; [`drawbar`](../drawbar) is a
browser app that drives that backend on hardware.

## More

- [`nord-usb` on docs.rs](https://docs.rs/nord-usb)
- [USB protocol][guide] — what is implemented and verified where, the frame
  layout, and the layering
- [Testing][testing] — the replay sweep, and what a capture has to contain

## Disclaimer

Not affiliated with, authorized, or endorsed by Clavia DMI AB. "Nord", "Clavia",
and "Electro" are trademarks of Clavia DMI AB, used here only to identify the
hardware this protocol belongs to. All reverse engineering is of traffic to and
from hardware the author owns, for interoperability.

[guide]: https://jmoo.github.io/drawbar/docs/reference/usb-protocol.html
[testing]: https://jmoo.github.io/drawbar/docs/reference/testing.html
