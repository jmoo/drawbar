# USB protocol

`nord-usb` speaks the vendor protocol Nord Sound Manager uses, reverse-engineered
from packet captures. It owns getting bytes on and off the instrument;
`nord-format` owns the bytes themselves.

> ⚠️ **Alpha software driving real hardware over a reverse-engineered protocol.**
> The operations listed below are hardware-verified; everything else is not. Back
> up your instrument — Nord Sound Manager makes a full backup — before pointing
> anything here at sounds you cannot re-create.

The wire protocol is decoded and validated. Implemented and **hardware-verified
on macOS**: inventory, object info, dependencies, the partition and bank
geometry, focus, the occupied-slot walk, program and set-list read/write, the
slot organization set (move, delete, rename, duplicate, select), reads and
in-place writes of the live slots (class 6) and the settings singleton (class 7),
and reads, deletes and writes of the piano (class 1) and sample (class 3)
libraries.

On **Linux** the read path and a multi-chunk sample write are hardware-verified.
The **WebUSB** backend is hardware-verified for reads and writes (Chrome on
macOS, via drawbar); three transport paths that only the browser takes are not.
**Windows** builds and passes the replay tests but has not been run against
hardware.

**Not implemented:** bundle and backup transfer, firmware update, and relink
(`0x35` — decoded from captures, never driven).

The integration tests replay real captures through the whole stack and assert
that the bytes the crate emits are the bytes NSM sent.

- [`nord-usb` on docs.rs](https://docs.rs/nord-usb)
- [`crates/nord-usb/README.md`](https://github.com/jmoo/drawbar/blob/master/crates/nord-usb/README.md) — the frame layout, the transport hazards, and the replay suite
