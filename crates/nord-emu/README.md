# nord-emu

An emulated **Clavia / Nord** instrument: the device side of the vendor USB
protocol, as an in-process state machine.

[`nord-usb`](../nord-usb) is the host half. This is something to point it at —
`EmuTransport` implements its `Transport` trait, so sessions, operations and
envelopes run against a simulated instrument with no hardware attached.

```rust
use nord_emu::{EmuDevice, EmuTransport, Object};
use nord_usb::{op, Location, ObjectClass, Session};

let mut device = EmuDevice::new();
device.insert(
    ObjectClass::Program,
    Location::from_user(7, 4),
    Object::new("Bright Grand", b"ne5p", 4, vec![0; 121]),
);

let mut t = EmuTransport::new(device);
let mut session = Session::open(&mut t, ObjectClass::Program).await?;
let info = op::info(&mut session, Location::from_user(7, 4)).await?;
session.commit().await?;
```

## Why, given `ReplayTransport` exists

A replay is a recording. It can only answer what a real device once answered, in
the order it answered, which is exactly what makes it the right tool for pinning
the bytes a host emits. It has nothing to say about **state**:

| | replay | emulator |
|---|---|---|
| the host's bytes match a capture | ✅ | ✅ |
| the device's bytes match a capture | — it supplies them | ✅ |
| a delete makes the next read fail | | ✅ |
| a move swaps two slots and rewrites the set lists pointing at them | | ✅ |
| a write into an occupied slot is refused, and succeeds after a delete | | ✅ |
| a reply arrives out of turn, or never arrives at all | | ✅ |
| the instrument is wedged, stalled, or lying about being empty | | ✅ |

## What is modelled

Everything here comes from this project's own captures of its own instrument and
from behaviour measured on it. Each behaviour carries its provenance in the code;
where a shape has never been observed — what `DELETE` answers for an empty slot,
for instance — the emulator answers a documented neighbouring status through
`Unmodeled` rather than inventing protocol, and says so.

- Session and UI handshakes, and **both** of their wedges: an abandoned class
  session refusing everything with `0x12`, and an abandoned UI session answering
  *successfully* while reporting every slot in every class as empty.
- Partition and bank geometry (a Nord Electro 5's by default), per-class block
  counters, object info, dependency lists with the dangling-reference marker
  resolved at read time.
- Chunked reads; the write path with its empty-destination precondition;
  delete, rename, move (a swap, with reference fix-up), copy (an overwrite),
  select and focus.
- The enumeration cursor, and the mutation that poisons it until a power cycle.
- Unsolicited change notifications, stalled bulk endpoints, and the command that
  answers nothing at all and leaves the session unclosable.

Not modelled: bundles, backups and firmware update; the `(Native)` library views
as a second window onto one pool; the startup-sync commands NSM sends that no
`nord-usb` operation does; the schema-version migration a move inflicts on the
set lists it rewrites.

## Testing

The suites drive the real host-side code against the emulator and compare **both
directions** against captures — the host's messages, and the device's answers,
which a replay cannot check because it provides them.

```sh
cargo test -p nord-emu
```

## Disclaimer

Not affiliated with, authorized, or endorsed by Clavia DMI AB. "Nord", "Clavia",
and "Electro" are trademarks of Clavia DMI AB, used here only to identify the
hardware this protocol belongs to. All reverse engineering is of traffic to and
from hardware the author owns, for interoperability.
