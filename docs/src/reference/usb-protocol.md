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

A bundle is read with these primitives alone (`nord_usb::bundle`): a set list's
dependency rows name its programs by slot, a program's name its pianos and
samples, and each of those is found by name, since an object's info does not
carry the id programs know it by. A piano's or sample's info does carry when it
was last changed.

Not implemented: writing a bundle to the instrument, firmware updates, and
relink.

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
rather than inferred. And operations are generic primitives parameterized by
object class (1 piano, 3 sample, 4 program, 5 set list, 6 live, 7 settings), so
one `rename` or `move` command serves every class.

The instrument reads a message until a short packet ends it. A frame that is an
exact multiple of the packet size needs a zero-length packet after it, or the
device never answers.

The closing exchanges of a transaction clear the instrument's progress display.
Abandoning a transaction after a progress label has been sent leaves the device
stuck until it is power-cycled, which is why every session closes on the error
path.

## Writing over a slot

The instrument does not overwrite an occupied program, set list, sample or piano
slot in place, so a write there deletes the occupant first. drawbar and `nord`
both read the occupant back before the delete and write it back if the new write
fails. `nord` reads an occupant of over 1 MiB of body straight into its rescue
file in the working directory, `nord-rescued-<bank>-<slot>.<tag>`, and deletes
that file once the slot holds what it should; drawbar's handling is under
[Reading by range](data-model.md#reading-by-range). Live slots and settings are
written in place.

Writing settings reloads the selected program, so unsaved panel changes are
lost. After a write or rename succeeds, drawbar asks for the focus, and if the
panel is on the slot just written, selects it again so the keyboard plays the
new sound. Nothing is reloaded after a failed write. The instrument reports no
progress percentage; progress shows on its own display.

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

A write reads its file through the `FileSource` trait, one transfer chunk at a
time, so memory does not grow with the file. The body is read twice: once to
check its checksum before the first frame, and again to send it. If the bytes
change in between, the last chunk is held back and the write fails unfinished.
`envelope::verify` runs the first of those checks alone, for a caller that must
know the file is whole before it touches the instrument.

A read hands its file to the `FileSink` trait the same way: `op::read_into`
writes each chunk of the body as it arrives, behind the room its `CBIN` header
takes, and the header last, once the body's CRC-32 is known. It returns the
slot's object info and that CRC-32. `op::read_program` is `read_into` into
memory, so the two send the same frames. A sink that keeps nothing,
`std::io::sink()`, takes a slot's checksum without holding its body.

The device reports a body CRC-32 in object info for programs and set lists, and
`0xffffffff` for pianos, samples and Settings, so the only way to learn what a
piano or sample slot holds is to read it. Its object info does report the body's
length, format tag, version and name.

The API documentation is on [docs.rs](https://docs.rs/nord-usb), and
[Testing](testing.md) covers the replay suite.
