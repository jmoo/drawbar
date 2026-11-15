# Your instrument

Back up your instrument before you send anything. Nord Sound Manager makes a full
backup. [What is supported](../getting-started/support.md) says which instruments
have been tested.

## Connecting

Close Nord Sound Manager first, since it keeps the USB connection to itself. Then
click **Connect instrument…** in the top bar, or choose **Instrument ▸
Connect…**. A web browser asks you to pick the device. The desktop app takes the
first Nord it finds. [What is supported](../getting-started/support.md#usb)
lists the browsers that can connect.

Once connected, drawbar reads the instrument's folders, and its slots appear,
labeled the way the panel shows them: `7:4 Africa Split`. Empty slots are listed
too, as places to drop things. **Instrument ▸ Read everything** (⌘R) reads them
again. A folder drawbar cannot name keeps the instrument's name for it and is read
only.

## Slots

Right-click a slot to **Open**, **Copy to this computer**, **Load on instrument**,
**Rename**, **Duplicate** or **Delete…**. Load on instrument makes the panel play
that slot, and a document from a slot has the same button in its
[header](editing.md#the-document-header). The name is typed in the browser, which
opens at the slot when you rename from the Keyboard tab or the Library.

Samples and pianos keep the name they were sent with, so they cannot be renamed
or duplicated on the instrument. The instrument also keeps one of each name: a
sample or piano named like one already there goes on Sync to replace it, and
cannot go to any other slot until you rename it.

With several slots checked, **Copy to this computer** reads each folder's slots
in one go. **Export as bundle…**
writes them, with what they play, as one bundle; see
[Bundles](this-computer.md#bundles).

Drag to move things:

- a slot onto **This computer** copies it;
- a sound onto a slot places it there for Sync;
- a slot onto another slot in the same folder swaps the two.

## Sync

Nothing is written until you sync. **Sync** in the top bar, or ⇧⌘S, reads the
instrument and lists what differs. `↑` writes a sound to the instrument, `↓`
brings one here; click the arrow to turn it. A `!` row waits for your pick. When
an instrument attaches with changes of its own, the list opens by itself.

A sound's name goes with it, minus the extension: `Africa-Split.ne5p` becomes
`Africa-Split` on the panel. Writing a sound renamed on this computer renames its
slot, even when the slot already holds the same sound.

Library undo, or **Undo** in the activity log, puts back a file Sync brought in, or
a link it moved or removed.

## Safety

- A move is a swap. Nothing is lost.
- Before writing over a sound, drawbar copies the old one. If the write fails,
  it puts the old one back. If that fails too, the old sound is kept on this
  computer, and the activity log says where.
- In the browser, a page left open while drawbar.app is updated cannot copy the
  old sound, so it writes over none until you reload it. Sending to an empty
  slot still works.
- An interrupted transfer cannot leave the instrument stuck on its progress
  screen.
- Writing Settings, or the slot the panel is playing, reloads the panel, so
  unsaved panel changes are lost. drawbar warns you before writing Settings.

## Disconnecting

Unplug, and the instrument's slots disappear. Sounds you placed on a slot stay
listed, so plug back in and sync.
