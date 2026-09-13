# The instrument

**Connect instrument**, beside Open… and New, opens the device (a chooser in the
browser, the first attached Clavia natively) and reads what it holds; the instrument's
own column appears with it, and goes away again when it is released. Each folder is read in **one session**,
the way Nord Sound Manager does it — the session is opened once and every slot
read inside it — so connecting no longer walks the instrument's display through
an open and a close per bank. Names fill in as each bank lands, and the folder
heading says how far it has got. Nothing asks for a bank number. **Read again**
re-walks one folder; anything you change is re-read on its own, and a change made
on the panel while the app is attached drops every cached name and reads them
again.

A folder is **one flat list**, each row labelled the way the panel and the CLI
label a slot — `7:4  Africa Split`. Empty slots are rows, not absences: they are
places something can be dropped. Pianos are listed and nothing more — a piano
library is hundreds of megabytes, and the folder offers no way to pull one down.

Writing into the slot the instrument currently has loaded leaves the panel
playing what it read before the write, so after a put or a rename that touches
it, the app asks the instrument to load that slot again. It can only do this for
a slot **it** selected: a selection made on the panel itself is invisible to the
host.

**An instrument that goes away says so.** Pull the cable and the column goes with
it — in a browser the moment the tab is told, and on either target the first time
an operation finds nothing on the other end of the pipe. A refusal is not a
disconnection: the instrument answering "no" is an instrument that is still
there. What was waiting to be sent stays waiting, so plugging back in and
pressing **Send all** picks up where you were.

## Moving things

Drag between the two places, or within one:

- **instrument → This computer** copies it here.
- **This computer → a slot** sends it. An occupied slot asks once — *Replace
  “Squabble B” in Programs 7:4 with “Africa Split”?* — and an empty one just
  does it.
- **slot → slot, inside one folder** rearranges them. The instrument swaps the
  two, so nothing is lost and nothing asks.

A target that cannot take what you are dragging does not light up; drop on it
anyway and the status strip says why.

Clicking anywhere in a row selects it — the whole row is the target, not the
words in it.

Right-click a row for Open, Copy to this computer / Export…, Load on instrument,
Rename, Duplicate, Delete… and Remove from list. Click the **name** of a row that
is already selected — or press F2 — to rename it in place; **Enter** renames and
anything else leaves it alone. Renaming on the instrument happens straight away,
because renaming it back is its own undo.

Progress is painted on the *instrument's own display* by the operations
themselves — there is no host-side progress callback, so the app shows a spinner
and does not invent one.

## Safety

**Back up your instrument first.** Nord Sound Manager makes a full backup.

- **Read-only until you say otherwise.** There is no armed mode: a destructive
  session exists only for the single operation the app released.
- **Replacing and deleting ask first, by name.** Nothing else does — a move
  loses nothing and a rename undoes itself.
- **Move is a swap, not an overwrite** — the destination's occupant ends up in
  the source slot, byte-identical. Confirmed on hardware.
- **A put is a delete followed by a write** — the instrument refuses to overwrite
  in place. The occupant is read into memory first and written back if the write
  fails; if the restore fails too, its bytes land on This computer as a rescued
  entity rather than being lost.
- **Live and settings are the exception: they overwrite in place**, so nothing is
  deleted to make room. Confirmed on hardware. The occupant is still read back
  first and restored the same way. Neither stores a name, so a write into one
  changes the body and leaves the slot called what it was called.
- **Writing settings reloads the selected program**, losing panel state that has
  not been stored. Confirmed on hardware, and the question asked before the write
  says so.
- **Every session closes, including on the error path.** An abandoned transaction
  strands the instrument on its progress screen with no way out but a power
  cycle.
- **One operation at a time.** What you asked for always goes ahead of the
  background read of the tree, so a click never waits on it. Reading a whole
  folder is one operation, and its progress arrives while it is still running.

## Sending changes back

Editing something you copied off the instrument does not write to it. The document is
marked **pending** instead — the tab and its row say *will be sent to Programs 7:4* —
and it goes nowhere until you say so.

- **Send all (n)**, beside the instrument's name, writes everything waiting. It asks
  once, listing every destination and naming what each one replaces, and then writes
  each folder's worth inside a single session. Progress arrives item by item.
- **Send to Programs 7:4** in a document's header sends that one, for when a batch is
  more ceremony than the job needs.
- If an item is refused the batch stops there. What was already written stays written,
  the report says how far it got, and the rest stay pending.
- **Cmd/Ctrl+S** marks the open document pending. For something that only lives on this
  computer it says so and does nothing, because those edits are already kept. It is
  never a file-export shortcut — that is Export….

**The name goes with the bytes.** The write carries the sound's local name, with the
format tag removed: `Africa-Split.ne5p` on this computer is `Africa-Split` on the
panel. An unnamed sound keeps an occupied slot's current name. If a write fails,
restoring the occupant also restores its name.
