# nord-usb

**Talk to a Nord keyboard over USB from Rust.** The protocol Nord Sound Manager
speaks, worked out from captures, with backends for the desktop (`nusb`), the
browser (`web`, over WebUSB), and recorded captures (`replay`, no hardware
needed).

> ⚠️ Alpha software driving real hardware. Back up your instrument first; Nord
> Sound Manager makes a full backup.
> [What is supported](../../docs/src/getting-started/support.md) says which
> operations have been verified on an instrument.

## Usage

```rust
use nord_usb::{op, Device, Location, ObjectClass};
use nord_usb::transport::UsbTransport;

let mut device = Device::new(UsbTransport::open_first()?);

// The instrument's own numbering: 7:4 on the panel.
let at = Location::from_user(7, 4);

// `read` hands the closure a read-only session. A write does not type-check here.
let (info, file) = device
    .read(ObjectClass::Program, async |s| {
        Ok((op::info(s, at).await?, op::read_program(s, at).await?))
    })
    .await?;
```

Writes go through `device.destructive(class, …)`, which hands the closure a
session that can write. Both brackets close the transaction whether the closure
succeeds or fails. An abandoned transaction would leave the instrument stuck on
its progress screen until it is power-cycled.

## Learn more

- [docs.rs](https://docs.rs/nord-usb)
- [USB protocol](../../docs/src/reference/usb-protocol.md): what works where,
  the frame layout, and the layering.
- [Testing](../../docs/src/reference/testing.md): the replay suite.

## Building

`cargo test -p nord-usb --features replay` from `crates/` in `nix develop`.
Without `replay`, no wire encoding is tested. The `web` feature needs a flag that
`crates/.cargo/config.toml` supplies, so build it from `crates/`.

## Disclaimer

Not affiliated with, authorized, or endorsed by Clavia DMI AB. "Nord", "Clavia",
and "Electro" are trademarks of Clavia DMI AB, used here only to identify the
hardware this protocol belongs to. All reverse engineering is of traffic to and
from hardware the author owns, for interoperability.
