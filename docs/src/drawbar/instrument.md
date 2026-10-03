# Your instrument

Back up your instrument before you send anything. Nord Sound Manager makes a full
backup. [What is supported](../getting-started/support.md) says which instruments
have been tested.

## Connecting

Close Nord Sound Manager first, since it keeps the USB connection to itself. Then
click **Connect instrument…** in the top bar, or choose **Instrument ▸
Connect…**. A web browser asks you to pick the device. The desktop app takes the
first Nord it finds. [Install](../getting-started/install.md#in-the-browser)
lists the browsers that can connect.

Once connected, drawbar reads the instrument's folders, and its slots appear,
labeled the way the panel shows them: `7:4 Africa Split`. Empty slots are listed
too, as places to drop things. **Instrument ▸ Read everything** (⌘R) reads them
again.

## Slots

Right-click a slot to **Open**, **Copy to this computer**, **Load on instrument**,
**Rename**, **Duplicate** or **Delete…**. Load on instrument makes the panel play
that slot, and a document from a slot has the same button in its
[header](editing.md#the-document-header).

Drag to move things:

- a slot onto **This computer** copies it;
- a sound onto a slot queues it for sending;
- a slot onto another slot in the same folder swaps the two.

## Sending

Nothing is written until you send. Dropping a sound on a slot, **Queue for
sending**, and saving a sound that came from a slot all add to the queue. **Send**
in the top bar counts what is waiting.

Click **Send**, or press ⇧⌘S, to review the queue. Click a destination to change
it, click × to remove a row, and select a row to see what it would replace.
**Send all** writes everything. If the instrument refuses something, the rest
stays queued.

When sounds on this computer no longer match the slots they came from, a
**Queue** button appears beside **Send**. Click it to queue them all.

A sound's name goes with it, minus the extension: `Africa-Split.ne5p` becomes
`Africa-Split` on the panel.

## Safety

- A move is a swap. Nothing is lost.
- Before writing over a sound, drawbar copies the old one. If the write fails,
  it puts the old one back. If that fails too, the old sound is kept on this
  computer, and the activity log says where.
- An interrupted transfer cannot leave the instrument stuck on its progress
  screen.
- Writing Settings, or the slot the panel is playing, reloads the panel, so
  unsaved panel changes are lost. drawbar warns you before writing Settings.

## Disconnecting

Unplug, and the instrument's slots disappear. The send queue is kept, so plug
back in and send.
