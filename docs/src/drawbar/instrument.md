# The instrument

**Back up your instrument first.** Nord Sound Manager makes a full backup.

USB support has been tested on the Nord Electro 5 only. Other models may connect,
but nothing guarantees that they behave.

## Connecting

Close Nord Sound Manager first. It holds the instrument's vendor interface for
itself, and nothing else can attach alongside it: not drawbar, `nord-cli` or a
browser. In a browser, only Chrome and Edge can connect; see
[Build and run](build.md#browser-support).

Click **Connect an instrument…** in the browser, or **Instrument ▸ Connect…**. A
browser shows its own device chooser; the desktop app opens the first Clavia device
attached. The row reads *Looking for an instrument…* while it connects.

Once the instrument answers, its name appears in the title bar and in PLACES, and
the instrument controls appear: **Read** and **Send** in the toolbar, the Keyboard
tab, the send queue, the inspector's INSTRUMENT panels and the rest of the
Instrument menu.

The folders come from the instrument's own partition table. On an Electro 5 they
are Pianos, Samples, Programs, Set lists, Live and Settings. A partition drawbar
cannot name is listed under the instrument's name for it, marked **read only**.

## Reading

Connecting reads every folder. Each folder is read in one session, the way Nord
Sound Manager does it, so the instrument's display does not open and close once per
bank. Names fill in as each bank arrives, and a folder's row counts its progress.

- **Read** in the toolbar, or **Instrument ▸ Read everything** (⌘R), reads the whole
  instrument again.
- **Read again** on the Keyboard tab, **Read this folder again** in a folder's menu,
  or **Instrument ▸ Read Programs again** while a document copied from Programs is
  open, reads one folder.

Anything drawbar changes is read again on its own. A change made on the instrument
while it is attached drops every cached name and reads the instrument again.

A folder is listed by bank, and a slot is labelled the way the panel and `nord-cli`
label it: `7:4 Africa Split`. Empty slots are listed too, as places something can be
dropped.

## Working with slots

Right-click a slot for **Open**, **Copy to this computer**, **Load on instrument**,
**Rename**, **Duplicate** and **Delete…**. A folder's menu offers **Go to loaded**
when the instrument reports the slot its panel is on.

**Open** shows the slot as a view; see
[Files on this computer](this-computer.md#views-and-kept-sounds). Renaming a slot
happens straight away, because renaming it back is its own undo. **Delete…** asks
first, by name, and cannot be undone.

## Moving things

Drag between the two places, or within one:

- **A slot onto This computer** copies it here.
- **A sound onto a slot** queues it for that slot. Nothing is written until you send.
- **A slot onto another slot in the same folder** swaps the two. Nothing is lost, and
  nothing asks.

A target that cannot take what you are dragging does not light up. Drop on it anyway
and the activity log says why: for example, the folder holds a different kind of
thing, or the instrument does not take files of that format.

The Library, the Keyboard tab and the send queue take the same drags as the browser.

## The send queue

Sending is two steps: queue, then send. **Queue for sending** in a menu or the
Library footer, a drop on a slot, and the document header's **Queue send** all add
to the queue and open its page in the bottom dock. **Review send queue…** (⇧⌘S)
opens the page directly.

- A sound that came off a slot, or whose bytes the instrument already holds, is
  queued for that slot. Anything else takes the next free slot of its folder, and
  a full folder refuses it.
- A sound the attached instrument does not take is refused and never enters the
  queue; the log says why. When only some of a set fit, the action reads
  **Queue 6 of 9**.
- Each queued row shows its destination as a chip. Click the chip to pick another
  bank and slot, or drop the row on a Keyboard cell. **×** takes a row out of the
  queue; double-click a row to open its document.
- Pick a row to see what it would change: the fields that differ between this
  computer and the keyboard, *the slot is free*, or *the instrument already holds
  these bytes*. A slot nothing has read is read first.

An edit does not queue itself. The header line counts `N queued · M changed · K
unsaved`, and **Queue N changed** queues every saved sound that differs from its
slot.

**Send all**, on the queue page, in the Instrument menu, or as **Send** in the
toolbar, asks once. The question lists every destination and what each one
replaces, with any warning first. Each folder's share is then written inside a
single session, item by item.

- Every entry is checked again against the instrument attached now. One it refuses
  keeps its place, with the reason, and is left out.
- If the instrument refuses an item, that folder's batch stops there. What was
  already written stays written, and the rest stay queued.
- **Clear**, or **Instrument ▸ Clear send queue**, empties the queue.
  **Instrument ▸ Remove from queue** removes what is picked.

**The name goes with the bytes.** A write carries the sound's name without its
format tag: `Africa-Split.ne5p` on this computer is `Africa-Split` on the panel. A
sound with no name keeps an occupied slot's current name.

## Safety

- **Read-only until you say otherwise.** There is no armed mode: a destructive
  session exists only for the single operation drawbar is carrying out.
- **Replacing and deleting ask first, by name.** A replacement asks only when the
  write carries a warning, such as a file format unlike everything read in that
  folder. Otherwise the queue has already shown the occupant and the difference.
- **A move is a swap, not an overwrite.** The destination's occupant ends up in the
  source slot, byte-identical. Confirmed on hardware.
- **A write into an occupied slot is a delete followed by a write,** because the
  instrument refuses to overwrite in place. The occupant is read into memory first,
  and nothing is deleted until that read succeeds. If the write fails, the occupant
  is written back; if that fails too, its bytes land on This computer as a rescued
  sound rather than being lost.
- **Live and Settings are the exception: they overwrite in place,** so nothing is
  deleted to make room. Confirmed on hardware. The occupant is still read back first
  and restored the same way. Neither stores a name, so a write changes the body and
  leaves the slot's name as it was.
- **Writing Settings reloads the selected program,** losing panel changes that have
  not been stored. Confirmed on hardware, and the question before the write says so.
- **Writing the loaded slot reloads it.** After a write or rename that touches the
  slot the panel is playing, drawbar asks the instrument to load it again. Confirmed
  on hardware. It can do this only for a slot drawbar itself selected with **Load on
  instrument**; a selection made on the panel is invisible to the host.
- **Every session closes, including on the error path.** An abandoned transaction
  strands the instrument on its progress screen with no way out but a power cycle.
- **One operation at a time.** What you ask for goes ahead of the background read,
  so a click never waits on it.

Progress is shown on the instrument's own display by the operations themselves.
There is no progress callback to the host, so drawbar shows a spinner in the status
bar and does not invent a percentage.

## When the instrument goes away

Pull the cable and the instrument's rows and controls go with it. A browser notices
at once; either target notices the first time an operation finds nothing on the
other end. A refusal is not a disconnection: an instrument that answers "no" is
still there.

The send queue survives a disconnection. Connect again, review the queue, and send.
