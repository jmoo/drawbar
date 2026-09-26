# Your instrument

Back up your instrument before you send anything. Nord Sound Manager makes a full
backup. [What is supported](../getting-started/support.md) says which instruments
have been tested.

## Connecting

Close Nord Sound Manager first, since it keeps the USB connection to itself. Then
click **Connect an instrument…** in the browser, or **Instrument ▸ Connect…**. The
browser asks you to pick the device. The desktop app takes the first Nord it
finds.

In a browser that cannot connect, these and the welcome sheet's first card are
greyed out, and hovering one says why.
[Install](../getting-started/install.md#in-the-browser) lists the browsers that
can.

Once connected, drawbar reads every folder the instrument declares, a bank at a
time, and the instrument's controls appear: **Read** and **Send** in the toolbar,
the Keyboard tab, the send queue, and the inspector's room meters. **Read**
rereads everything, and each folder has its own **Read again**.

Slots are labelled the way the panel shows them, `7:4 Africa Split`. Empty slots
are listed too, as places to drop things.

## Slots

Right-click a slot to **Open**, **Copy to this computer**, **Load on instrument**,
**Rename**, **Duplicate** or **Delete…**. Rename happens straight away, because
renaming back is its own undo. Delete asks first.

Drag to move things:

- a slot onto **This computer** copies it;
- a sound onto a slot queues it for sending;
- a slot onto another slot in the same folder swaps the two, losing nothing.

A slot that cannot take what you are dragging does not light up. Drop anyway and
the log says why.

## The send queue

Nothing is written until you send. Drops, **Queue for sending**, and Save on a
sound that belongs to a slot all add to the queue in the bottom dock. Each row
shows its destination: click it to change it, × to remove the row, and pick a row
to see what would change on the keyboard.

When a sound on this computer no longer matches the slot it came from, a **Queue**
button appears in the toolbar beside **Send**, with how many there are. Click it
to queue each of them for the slot it stands on.

**Send all** asks once, listing every destination and what it replaces, then
writes folder by folder. If the instrument refuses an item, that folder's batch
stops there. What was written stays, and the rest stays queued. The queue also
survives a disconnection.

A sound's name goes with it, minus the file extension: `Africa-Split.ne5p` on
this computer becomes `Africa-Split` on the panel.

## Safety

- drawbar never leaves a write session open. Each write opens one, finishes, and
  closes it, so an interrupted transfer cannot leave the instrument stuck on its
  progress screen.
- A move is a swap, never an overwrite.
- Writing into an occupied slot deletes the old sound first, because the
  instrument refuses to overwrite in place. drawbar reads the old sound into memory
  before deleting it, writes it back if the new write fails, and if that fails too,
  keeps its bytes on this computer as a rescued sound.
- Live slots and Settings are overwritten in place. Writing Settings reloads the
  selected program, so unsaved panel changes are lost. drawbar warns before it does
  this.
- When a write or rename succeeds, drawbar asks the panel which slot it is
  playing. If that slot was just written, drawbar loads it again so the keyboard
  plays the new sound, and unsaved panel changes to it are lost. A panel on any
  other slot is left alone, and nothing is reloaded after a failed write.

These behaviours have been confirmed on an instrument. Progress shows on the
instrument's own display; drawbar shows a spinner and cannot know a percentage.

## Disconnecting

Unplug, and the instrument's rows and controls disappear. The send queue is kept,
so plug back in, review it, and send.
