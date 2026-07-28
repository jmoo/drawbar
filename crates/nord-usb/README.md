# nord-usb

Talk to **Clavia / Nord** keyboards over USB from Rust — the vendor protocol Nord
Sound Manager speaks, reverse-engineered from packet captures.

This is the transport-and-protocol half of the Nord toolkit.
[`nord-format`](../nord-format) owns the bytes of a file; this crate owns getting
those bytes on and off the instrument. It depends on `nord-format` for the
container it wraps read data in, and on nothing else at its core — the backends
are optional features.

## Layering

The protocol is testable without hardware, which is the whole point of the split:

| Module | Role |
|---|---|
| `wire` | Message framing and codec. Pure, no I/O. |
| `transport` | The byte pipe. The **only** part that touches a device. |
| `session` | The transaction wrapper every operation runs inside. |
| `op` | Typed operations. |

## The protocol

Every message on the vendor bulk endpoints is a length-prefixed, CRC-trailered
frame of **big-endian** `u32`s (the *file* formats are little-endian — mixing them
up is an easy afternoon lost):

```
┌────────┬─────────┬───────────┬─────────┬───────────────┬───────┐
│ length │ service │ subsystem │ command │ args…         │ crc16 │
│  u32   │   u32   │    u32    │   u32   │               │  u16  │
└────────┴─────────┴───────────┴─────────┴───────────────┴───────┘
```

The CRC is **CRC-16/CCITT-FALSE**. A response is the request's command `+ 1` with
a `u32` status inserted ahead of the echoed arguments, which is why responses run
exactly four bytes longer.

**Verified against all 4,589 messages in the capture corpus** — 100% CRC match,
100% length-field match.

Two things that cost real time and are worth knowing up front:

- **Requests are not reliably even.** `SELECT` is `0x2f` with response `0x30`.
  Direction is the only dependable discriminator, so this crate records it at
  decode time rather than inferring it. Getting that wrong misaligns every
  argument by four bytes and hides device error codes.
- **Operations are primitives parameterised by an object class**, not per-type
  opcodes. `SESSION_OPEN` carries the class (1 piano, 3 sample, 4 program, 5 set
  list) and the same `rename` / `move` / `delete` / `copy` commands then apply to
  whichever it is.

## Usage

```rust
use nord_usb::{op, ObjectClass, Session};
use nord_usb::transport::UsbTransport;

let mut transport = UsbTransport::open_first()?;

// Read-only by default — the type system will not let a mutating op through.
let mut session = Session::open(&mut transport, ObjectClass::Program).await?;
let info = op::info(&mut session, "7:4".parse()?).await?;
let file = op::read_program(&mut session, "7:4".parse()?).await?;
session.commit().await?;
```

Mutating operations need the capability to be asked for explicitly:

```rust
let mut session = Session::open(&mut t, ObjectClass::Program)
    .await?
    .allow_destructive_writes();
op::delete(&mut session, at).await?;
session.commit().await?;
```

**Always `commit()`, including on the error path.** The closing exchanges are what
clear the instrument's progress display; abandoning a transaction after a progress
label has been sent leaves the device stuck until it is power-cycled. `Session`
carries a `Drop` assertion to catch the mistake in debug builds.

## Features

| Feature | Default | What it gives you |
|---|:--:|---|
| `nusb` | ✅ | Desktop backend — macOS (IOKit), Linux (usbfs), Windows (WinUSB). Pure Rust. |
| `web` | | Browser backend over WebUSB. Chrome/Edge only; Firefox and Safari declined the spec. |
| `replay` | | Drive the protocol from committed captures, no hardware. Used by the golden tests. |
| `blocking` | | Block on the async API from synchronous callers (the CLI). Tiny; not a runtime. |
| `corpus` | | Corpus-backed tests (`NORD_CORPUS_DIR`), implies `replay`. |

### Portability

WebUSB is the binding constraint on the API shape. Its handles are not `Send`, so
neither is this crate's `Transport` trait — which in turn keeps it
runtime-agnostic. Device *enumeration* is deliberately backend-specific rather
than part of the portable core, because the browser requires a user gesture to
pick a device and no portable signature can express that.

`block_on` exists for CLIs and tests that just want the answer, without pulling in
a full async runtime.

## Testing

The golden tests replay real captures through the whole stack and assert the
bytes this crate emits are **the bytes NSM sent** — not merely self-consistent
with its own encoder. No hardware, no platform dependency.

```sh
cargo test -p nord-usb --features replay
```

⚠️ `replay` is not a default feature, so a bare `cargo test -p nord-usb` compiles
the golden tests out and reports a pass having verified none of the wire encoding.
The Nix build enables it via `[package.metadata.nix] testFeatures` in `Cargo.toml`.

## Status

> [!CAUTION]
> # 🚫 DO NOT USE THIS IN ITS CURRENT STATE 🚫
>
> This crate drives real hardware over a reverse-engineered protocol and is
> pre-1.0. Do not point it at a Nord device you care about.

The wire protocol is decoded and validated. Implemented and hardware-verified on
macOS: inventory, object info, dependencies, program read/write, and the slot
organisation set (move, delete, rename, duplicate, select).

Not implemented: bundle and backup transfer, firmware update, and the piano/sample
library as first-class objects. Linux and Windows build and pass the replay tests
but have not been run against hardware.

## Disclaimer

Not affiliated with, authorized, or endorsed by Clavia DMI AB. "Nord", "Clavia",
and "Electro" are trademarks of Clavia DMI AB, used here only to identify the
hardware this protocol belongs to. All reverse engineering is of traffic to and
from hardware the author owns, for interoperability.
