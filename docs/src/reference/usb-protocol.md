# USB protocol

`nord-usb` speaks the protocol Nord Sound Manager uses, worked out from USB
captures. It moves bytes to and from the instrument. `nord-format` owns the
bytes.

## What works

Verified on an instrument from macOS: inventory, object info, dependencies,
geometry, focus, walking the occupied slots, reading and writing programs and
set lists, moving, deleting, renaming, duplicating and selecting slots, reading
and writing live slots and settings in place, and reading, deleting and writing
sample and piano libraries. From Linux, reads and a multi-chunk sample write are
verified, and every recorded request is byte-identical to its macOS counterpart.
The WebUSB backend is verified for reads and writes from Chrome. Windows passes
the replay tests but has not been run against an instrument.

Not implemented: backup bundles, firmware updates, and relink.

## The wire

Every message is a length-prefixed frame of big-endian 32-bit words with a
CRC-16 trailer:

```
┌────────┬─────────┬───────────┬─────────┬───────────────┬───────┐
│ length │ service │ subsystem │ command │ args…         │ crc16 │
└────────┴─────────┴───────────┴─────────┴───────────────┴───────┘
```

The CRC is CRC-16/CCITT-FALSE. A response carries the request's command plus
one, with a status word inserted before the echoed arguments. Two things bite.
Request codes are not reliably even, so direction is recorded at decode time
rather than inferred. And operations are generic primitives parameterised by
object class (1 piano, 3 sample, 4 program, 5 set list, 6 live, 7 settings), so
one `rename` or `move` command serves every class.

The instrument reads a message until a short packet ends it. A frame that is an
exact multiple of the packet size needs a zero-length packet after it, or the
device never answers.

The closing exchanges of a transaction clear the instrument's progress display.
Abandoning a transaction after a progress label has been sent leaves the device
stuck until it is power-cycled, which is why every session closes on the error
path.

## Layering

| Module | Role |
|---|---|
| `wire` | Framing and codec. No I/O. |
| `transport` | The byte pipe, and the only code that touches a device. |
| `session` | The transaction every operation runs inside. |
| `op` | Typed operations. |
| `device` | An instrument as a value: session brackets and its geometry. |

## Features

| Feature | Default | Gives |
|---|:--:|---|
| `nusb` | yes | Desktop backend for macOS, Linux and Windows, in pure Rust. |
| `web` | | Browser backend over WebUSB. Chrome and Edge only. |
| `replay` | | Drive the protocol from captures, with no hardware. |
| `blocking` | | Block on the async API from synchronous code. |
| `corpus` | | Tests against the private capture corpus. Implies `replay`. |
| `fault-injection` | | Deliberate protocol faults for research. A wrong value can stall the instrument's endpoints. |

WebUSB handles are not `Send`, so neither is the `Transport` trait, which keeps
the crate free of any particular async runtime. Building `web` needs
`--cfg=web_sys_unstable_apis`, which `crates/.cargo/config.toml` supplies when
Cargo runs from `crates/`.

The API documentation is on [docs.rs](https://docs.rs/nord-usb), and
[Testing](testing.md) covers the replay suite.
