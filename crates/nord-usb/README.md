# nord-usb

Talk to **Clavia / Nord** keyboards over USB from Rust — the vendor protocol Nord
Sound Manager speaks, reverse-engineered from packet captures.

This is the transport-and-protocol half of the Nord toolkit.
[`nord-format`](../nord-format) owns the bytes of a file; this crate owns getting
those bytes on and off the instrument. It depends on `nord-format` for the
container it wraps read data in, and on nothing else at its core — the backends
are optional features.

> [!WARNING]
> Alpha software driving real hardware over a reverse-engineered protocol.
> The verbs listed under **Status** are hardware-verified; everything else is
> not. Back up your instrument (Nord Sound Manager makes a full backup) before
> pointing anything here at sounds you can't re-create.

## Status

The wire protocol is decoded and validated. Implemented and hardware-verified on
macOS and Linux: inventory, object info, dependencies, program read/write, the
slot organization set (move, delete, rename, duplicate, select), and reads of the
live slots (class 6) and the settings singleton (class 7). Linux emits
byte-identical request frames to macOS for every verb.

Those two classes take a write **in place**, at an occupied slot, which no other
class does — confirmed on hardware by driving the sequence directly. `op::write`
has no in-place branch, so it still composes a write as delete-then-write and
refuses class 6 and 7 rather than delete them; whether either survives a delete of
its own class is unconfirmed on hardware.

The WebUSB backend is hardware-verified for reads and writes (Chrome on macOS,
via `drawbar`). Three transport paths that only the browser takes are not:
`select_configuration` on a device the OS left unconfigured, multi-chunk bulk
reads, and a `transferIn` whose payload is an exact multiple of the packet size.

Frames are terminated: the instrument reads a message until a **short** packet
ends it, so a frame whose length is a whole multiple of the OUT endpoint's
`wMaxPacketSize` is followed by a zero-length packet. Without it the device
never answers and the session is stranded — a `RENAME` carrying a 34-character
name is exactly 64 bytes on this full-speed link, and 33 characters is not.

Not implemented: bundle and backup transfer, firmware update, and the piano/sample
library as first-class objects. Windows builds and passes the replay tests but has
not been run against hardware.

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
| `web` | | Browser backend over WebUSB. Chrome/Edge only — Firefox and Safari declined the spec. |
| `replay` | | Drive the protocol from committed captures, no hardware. |
| `blocking` | | Block on the async API from synchronous callers (the CLI). Tiny; not a runtime. |
| `corpus` | | Corpus-backed tests (`NORD_CORPUS_ROOT`), implies `replay`. |

WebUSB is the binding constraint on the API shape. Its handles are not `Send`, so
neither is this crate's `Transport` trait — which in turn keeps it
runtime-agnostic. Device *enumeration* is backend-specific rather than part of
the portable core, because the browser requires a user gesture to pick a device
and no portable signature can express that. `block_on` (the `blocking` feature)
exists for CLIs and tests that just want the answer, without pulling in a full
async runtime.

Building the `web` feature needs `--cfg=web_sys_unstable_apis` (WebUSB is still
gated in `web-sys`); `crates/.cargo/config.toml` supplies it for the wasm target,
so wasm builds must be run from `crates/` or below.
[`drawbar`](../drawbar) is a browser app that drives this backend on hardware.

## The protocol

Every message on the vendor bulk endpoints is a length-prefixed, CRC-trailered
frame of **big-endian** `u32`s (the *file* formats are little-endian — mixing
them up costs real debugging time):

```
┌────────┬─────────┬───────────┬─────────┬───────────────┬───────┐
│ length │ service │ subsystem │ command │ args…         │ crc16 │
│  u32   │   u32   │    u32    │   u32   │               │  u16  │
└────────┴─────────┴───────────┴─────────┴───────────────┴───────┘
```

The CRC is **CRC-16/CCITT-FALSE**. A response is the request's command `+ 1` with
a `u32` status inserted ahead of the echoed arguments, which is why responses run
exactly four bytes longer. Every message in the capture corpus decodes and
re-encodes with its CRC and length field intact.

Two hazards worth knowing up front:

- **Requests are not reliably even.** `SELECT` is `0x2f` with response `0x30`.
  Direction is the only dependable discriminator, so this crate records it at
  decode time rather than inferring it. Getting that wrong misaligns every
  argument by four bytes and hides device error codes.
- **Operations are primitives parameterised by an object class**, not per-type
  opcodes. `SESSION_OPEN` carries the class (1 piano, 3 sample, 4 program, 5 set
  list, 6 live, 7 settings) and the same `rename` / `move` / `delete` / `copy`
  commands then apply to whichever it is.

## Layering

The protocol is testable without hardware, which is the whole point of the split:

| Module | Role |
|---|---|
| `wire` | Message framing and codec. Pure, no I/O. |
| `transport` | The byte pipe. The **only** part that touches a device. |
| `session` | The transaction wrapper every operation runs inside. |
| `op` | Typed operations. |

## Testing

The integration tests replay real captures through the whole stack and assert the
bytes this crate emits are **the bytes NSM sent** — not merely self-consistent
with its own encoder. No hardware, no platform dependency.

```sh
cargo test -p nord-usb --features replay
```

⚠️ `replay` is not a default feature, so a bare `cargo test -p nord-usb` compiles
the replay tests out and reports a pass having verified none of the wire encoding.
The Nix build enables it via `[package.metadata.nix] testFeatures` in `Cargo.toml`.

### The replay sweep

`tests/replay` is one trial per `*.script` under `tests/scripts`, and — with
`--features corpus` and `NORD_CORPUS_ROOT` — under the private corpus too. A
capture joins the suite by existing; no test is written for it.

Every script is checked for framing. A script whose header declares an **intent**
is also driven: replayed through an exact-match transport, one section per
transaction, each judged against what it said to `expect`, and the whole script
required to be consumed.

```
# source: nord
# device: Nord Electro 5, firmware v2.04 build 592
# intent: program info 7:11
O 0000001200000006000000010000000006a1
…
# intent: program move 7:11 7:12
…
```

`nord … --record <path>` writes that header itself, so a capture made with the CLI
is a complete replay script. The full vocabulary — header keys, the `expect` values, and
the intent → operation table — is in
[`tests/scripts/README.md`](tests/scripts/README.md).

## Disclaimer

Not affiliated with, authorized, or endorsed by Clavia DMI AB. "Nord", "Clavia",
and "Electro" are trademarks of Clavia DMI AB, used here only to identify the
hardware this protocol belongs to. All reverse engineering is of traffic to and
from hardware the author owns, for interoperability.
