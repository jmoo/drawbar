# USB protocol

`nord-usb` speaks the vendor protocol Nord Sound Manager uses, reverse-engineered
from packet captures. It owns getting bytes on and off the instrument;
`nord-format` owns the bytes themselves — it depends on `nord-format` for the
container it wraps read data in, and on nothing else at its core, since the
backends are optional features.

> ⚠️ **Alpha software driving real hardware over a reverse-engineered protocol.**
> The verbs listed under Status are hardware-verified; everything else is not.
> Back up your instrument (Nord Sound Manager makes a full backup) before
> pointing anything here at sounds you can't re-create.

## Status

The wire protocol is decoded and validated. Implemented and hardware-verified on
macOS: inventory, object info, dependencies, the partition and bank geometry,
focus, the occupied-slot walk, program and set-list read/write, the slot
organization set (move, delete, rename, duplicate, select), reads and in-place
writes of the live slots (class 6) and the settings singleton (class 7), and
reads, deletes and writes of the piano (class 1) and sample (class 3) libraries —
a library write sizes the instrument's cleaning pass from `STATUS`, runs it in
the write's own session, and chunks the body. On Linux the read path and a
multi-chunk sample write are hardware-verified. Across 14 recorded read-only
commands, every request frame is byte-identical to its macOS counterpart.

`BEGIN_WRITE` is the only frame in a write that carries a name. The library
classes take their immutable name there because they refuse `rename`; live and
settings discard it, just as they accept but ignore `rename`.

Those two classes overwrite an occupied slot in place
(`ObjectClass::overwrites_in_place`), so a caller must not compose their write out
of delete-then-write — deleting either has never been attempted.

The WebUSB backend is hardware-verified for reads and writes (Chrome on macOS,
via [`drawbar`](../drawbar/overview.md)). Three transport paths that only the
browser takes are not: `select_configuration` on a device the OS left
unconfigured, multi-chunk bulk reads, and a `transferIn` whose payload is an
exact multiple of the packet size.

Frames are terminated: the instrument reads a message until a **short** packet
ends it, so a frame whose length is a whole multiple of the OUT endpoint's
`wMaxPacketSize` is followed by a zero-length packet. Without it the device
never answers and the session is stranded — a `RENAME` carrying a 34-character
name is exactly 64 bytes on this full-speed link, and 33 characters is not.

Not implemented: bundle and backup transfer, firmware update, and relink (`0x35` —
decoded from captures, never driven). Windows builds and passes the replay tests
but has not been run against hardware.

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

A transaction's closing exchanges are what clear the instrument's progress
display: abandoning one after a progress label has been sent leaves the device
stuck until it is power-cycled.

## Layering

The protocol is testable without hardware, which is the whole point of the split:

| Module | Role |
|---|---|
| `wire` | Message framing and codec. Pure, no I/O. |
| `transport` | The byte pipe. The **only** part that touches a device. |
| `session` | The transaction wrapper every operation runs inside. |
| `op` | Typed operations. |
| `device` | An instrument as a value: session brackets and its own geometry. |

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
[drawbar](../drawbar/overview.md) is a browser app that drives this backend on
hardware.

## Further reading

- [`nord-usb` on docs.rs](https://docs.rs/nord-usb)
- [Testing](testing.md) — the replay suite, and what a capture has to contain
