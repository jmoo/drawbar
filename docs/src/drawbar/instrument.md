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
sample or piano named like one already there is queued to replace it, and
cannot go to any other slot until you rename it.

With several slots checked, **Copy to this computer** reads each folder's slots
in one go. **Export as bundle…**
writes them, with what they play, as one bundle; see
[Bundles](this-computer.md#bundles).

Drag to move things:

- a slot onto **This computer** copies it;
- a sound onto a slot queues it for sending;
- a slot onto another slot in the same folder swaps the two.

## Sync

Nothing is written until you sync. **Sync** in the top bar, or ⇧⌘S, reads the
keyboard and lists every sound that differs between it and this computer: files
newer here, slots changed on the keyboard, sounds moved, replaced or removed
there, and sounds you dropped on a slot or sent with **Send to…** or **Queue for
sending**. Its count is the number of rows.

Each row has a way: `↑` writes the file to its slot, `↓` brings the slot into the
file. Click the arrow to turn it. A sound changed on both sides, or removed on the
keyboard, has no way until you pick one. Click × to leave a row out, and select a
row to see the two versions side by side. **Sync** in the sheet does every row
that has a way. If the instrument refuses a write, that sound stays listed.

A program sent to the keyboard takes along the piano or sample it plays when only
this computer has it. A piano or sample is never brought in; copy it to this
computer instead.

When a keyboard attaches and something changed on it, the sheet opens by itself.

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

Unplug, and the instrument's slots disappear. Sounds you sent to a slot stay
listed, so plug back in and sync.
