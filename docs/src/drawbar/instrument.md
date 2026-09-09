# The instrument

**Connect instrument**, beside Open… and New, opens the device — a chooser in the
browser, the first attached Clavia natively — and reads what it holds. Each
folder (Programs, Set lists, Samples, Pianos, Live, Settings) is read in one
session, the way Nord Sound Manager does it, and appears as one flat list of
slots labelled the way the panel and the CLI label them: `7:4  Africa Split`.
Empty slots are rows, not absences.

Drag between the two places, or within one: **instrument → This computer** copies
it here, **This computer → a slot** sends it, and **slot → slot inside one
folder** rearranges them — the instrument swaps the two, so nothing is lost.

Editing something you copied off the instrument does not write to it. The
document is marked **pending** — its tab and row say *will be sent to Programs
7:4* — until **Send all** or the document's own **Send to Programs 7:4** writes
it. If an item is refused the batch stops there; what was written stays written
and the rest stay pending.

## Safety

- **Read-only until told otherwise.** A destructive session exists only for the
  single operation the app released.
- **Replacing and deleting ask first, by name.** Nothing else does — a move
  loses nothing and a rename undoes itself.
- **Every session closes, including on the error path.** An abandoned
  transaction strands the instrument on its progress screen with no way out but
  a power cycle.
- **Back up your instrument first.** Nord Sound Manager makes a full backup.

The whole behaviour, including what a write does to an occupied slot, is in
[`crates/drawbar/README.md`](https://github.com/jmoo/drawbar/blob/master/crates/drawbar/README.md).
