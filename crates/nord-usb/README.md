# nord-usb

**Talk to a Nord keyboard over USB from Rust.** The protocol Nord Sound Manager
speaks, worked out from captures.

- Backends for the desktop (`nusb`), the browser (`web`, over WebUSB), and
  recorded captures (`replay`, no hardware needed).
- Reads get a read-only session, so a write in one does not compile.
- Every session closes even when its closure fails, so the instrument is not
  left on its progress screen.
- Pianos and samples stream to and from a file a transfer chunk at a time.

> ⚠️ Alpha software driving real hardware: back up your instrument first.
> [What is supported](../../docs/src/getting-started/support.md) says what has
> been verified on an instrument.

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

## Learn more

- [docs.rs](https://docs.rs/nord-usb): the API, the protocol and the features.
- [USB protocol](../../docs/src/reference/usb-protocol.md) in the guide.
- [Testing](../../docs/src/reference/testing.md): the replay suite.

## Disclaimer

Not affiliated with, authorized, or endorsed by Clavia DMI AB. "Nord", "Clavia",
and "Electro" are trademarks of Clavia DMI AB, used here only to identify the
hardware this protocol belongs to. All reverse engineering is of traffic to and
from hardware the author owns, for interoperability.
