# USB protocol

`nord-usb` speaks the protocol Nord Sound Manager uses, worked out from USB
captures. It moves bytes to and from the instrument. `nord-format` owns the
bytes.

## What works

[What is supported](../getting-started/support.md#usb) says what has been
confirmed on which instrument and system.

Not implemented: writing a bundle to the instrument, firmware updates, and
relink.

## The crate documentation

The [nord-usb documentation](https://docs.rs/nord-usb/latest/nord_usb/) on
docs.rs describes the protocol beside the code that speaks it:

- [`wire`](https://docs.rs/nord-usb/latest/nord_usb/wire/): the frame layout and its CRC, the commands, and the object
  classes, which are found by the name the instrument gives each partition.
- [`transport`](https://docs.rs/nord-usb/latest/nord_usb/transport/): the backends, and the zero-length packet that
  ends a frame filling a whole number of packets.
- [`session`](https://docs.rs/nord-usb/latest/nord_usb/session/) and [`device`](https://docs.rs/nord-usb/latest/nord_usb/device/): the transaction every
  operation runs in, which closes even on an error so the instrument is not left
  on its progress screen.
- [`op`](https://docs.rs/nord-usb/latest/nord_usb/op/): the typed operations, the statuses the instrument refuses a
  request with, and reads and writes that stream a chunk at a time.
- [`bundle`](https://docs.rs/nord-usb/latest/nord_usb/bundle/): how the objects a bundle needs are found on the
  instrument.
- [Features](https://docs.rs/nord-usb/latest/nord_usb/#features): the backends, and the features for tests and
  research.

## Writing over a slot

The instrument does not overwrite an occupied program, set list, sample or piano
slot in place, so a write there deletes the occupant first. drawbar and `nord`
both read the occupant into a file under a `.partial` name, sync it, check its
length and checksum, and only then give it its own name, all before the delete.
A process that dies with the slot empty leaves the whole occupant on disk, and
one that dies during the read leaves only a `.partial` file and the slot as it
was. If the new write fails, the occupant is written back. Live slots and
settings are written in place.

What `nord` does with that file is under
[What a write does](../nord-cli/instrument.md#what-a-write-does), and what
drawbar does under [What this install keeps](data-model.md#what-this-install-keeps).

Writing settings reloads the selected program, so unsaved panel changes are
lost.

[Testing](testing.md) covers the replay suite.
