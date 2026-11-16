# RFC-0325: Links and Sync

- Start date: 2026-11-14
- Authors: jmoo
- RFC PR: [jmoo/drawbar#325](https://github.com/jmoo/drawbar/pull/325)
- Tracking issue: none; the RFC PR implements it

## Summary

A library file keeps a stored link to its slot on a keyboard, one link per keyboard,
where a keyboard is a model and a serial number. The link records the checksums the
file and the slot have shared, which gives every difference a direction: newer here,
newer on the keyboard, or changed on both. One action, Sync, reads the keyboard and
lists what differs, one row per sound with a direction the user can turn, and then
runs exactly what it listed. What a program plays is worked out from names, because a
piano's or sample's id is a hash of its name, so no relation is stored.

## Motivation

Before this change, at
[`aef7dc79`](https://github.com/jmoo/drawbar/commit/aef7dc7942313ea887ab6c4e46080e0d3314ad3c),
drawbar guesses the slot a file belongs to each time it looks, from equal bytes and
from the slot the file was copied off. Nothing is stored. The guess fails in ordinary
use:

| What the user does | What happens |
|---|---|
| Makes a New program | Every New program has the same bytes. Where a slot holds those bytes, the new file claims it, and sending proposes replacing it |
| Sends a file to another slot | The file still shows the slot it matched before, also after a restart |
| Copies a slot and sends the copy elsewhere | The copy stays on the slot it came off, reading as different |
| Moves or deletes a slot at the panel | The file jumps to any unrelated slot holding equal bytes |
| Edits a slot at the panel | The file reads as different, and the only remedy sends the older copy over the panel edit |
| Duplicates a file, or imports one twice | Every copy claims the slot, and saving any of them queues a send to it |
| Imports a bundle | Which piano a program plays is known only while the keyboard is attached |

The causes are few. Equal bytes are treated as identity. An address is treated as
identity with no memory of what it held. A difference has no direction. Each surface
reads "which slot" in its own way. A player with one keyboard and a folder of sounds
cannot tell from the screen which side is newer, and one wrong send loses work.

## Guide-level explanation

### What a user sees

A linked file shows its slot's address in the library. When the file and the slot
agree, that is all it shows. Otherwise a mark stands before the name:

- `↑` the file is newer;
- `↓` the slot is newer;
- `!` it waits for a pick, or its link is about to change.

**Sync** in the top bar counts what differs. Pressing it reads the keyboard and opens
one sheet:

```text
Sync with Nord Electro 5

↑  Tine         8:3   newer here
↓  Reed Trem    8:4   newer on the keyboard
!  Organ Pad    8:5   both changed
↓  Clav Comp    8:6   moved to 8:9
↑  Upright      new · “Tine” plays it
```

`↑` writes the file to its slot. `↓` brings the slot's version into the file. The
arrow is a control: clicking it turns the row. A `!` row does nothing until the user
picks a direction (both changed, removed on the keyboard). A sound moved on the
keyboard takes its link along, and one replaced there offers `Unlink`; neither row
changes a sound. Nothing is written or brought in until the sheet's button is pressed.
When a keyboard attaches and its side has changed, the sheet opens by itself.

Links are made without asking:

- copying a slot to this computer links the copy to it;
- a send that lands links the file to the slot it went to;
- on attach, a file with no link on that keyboard is linked to the slot its header
  names, or the slot it was copied off, when that slot holds its exact bytes and no
  other file is linked there;
- a move, rename or delete drawbar makes on the keyboard takes the link along.

**Link to ›** and **Unlink** in the row menu change a link by hand. An unlinked file
stays off that keyboard until the user links or sends it again. A slot that holds a
file's exact bytes without a link is a hint in the Selection card and nothing more.

A file sent to a slot that is not its own is placed there by hand and listed in the
same sheet. A program going up lists the piano or sample it plays as a row of its
own when the keyboard lacks it.

Library undo, or Undo in the activity log, takes back a link made, moved or removed,
and a file Sync brought in. A write to the keyboard has no undo.

### What a contributor sees

`drawbar::link` is the contract. `standing(held, saved, there)` compares a link, the
file's saved checksum and what a read of the keyboard reported for the slot, and
returns `Same`, `Ahead`, `Theirs`, `Both`, `Gone` or `Unread`. `drawbar::bring::rows`
turns the standings, the hand-placed sends and what programs play into the sheet's
rows. The library's marks, the Sync count and the sheet all read those rows, so they
cannot disagree.

## Reference-level explanation

### The link

The library stores `links`, a set per file with one member per keyboard:

```rust
struct Keyboard { model: String, serial: Option<String> }
struct Link { keyboard: Keyboard, held: Option<Held> }
struct Held { class, spot: Spot, name: String, base: Print, past: Vec<u32> }
enum Spot { At { bank, slot }, Named(String) }
struct Print { crc: u32, len: u32 }
```

- **Keyboard.** One that reports no serial is every keyboard of its model. The
  Electro 5 reports none. Confirmed on hardware.
- **Spot.** Programs, set lists, live slots and settings are linked by address. A piano
  or sample is linked by the name the keyboard keeps it under, since the keyboard holds
  one of each name wherever it sits.
- **Base and past.** `base` is the checksum and length the file and the slot last
  agreed on. `past` holds up to eight earlier checksums of the slot. A slot's checksum
  covers its content only: renaming a slot keeps it, and moving a slot carries it.
  Confirmed on hardware. Slot info carries no time, so the checksums are the only
  history there is. Confirmed on hardware.
- **Name.** The name an addressed slot had at the last agreement. It tells a changed
  sound from a different sound in the same slot.
- **`held: None`** records an Unlink, so the automatic rules leave the file alone on
  that keyboard.

One file is linked to a slot. Two computers can each link a different file to one
slot before their libraries merge. One pass then keeps the file that holds the slot's
sound, then the one whose link knew the slot's checksum, then the one recorded under
the slot's name, and unlinks the rest, with Undo.

### Standing

| Slot holds | File | Standing | Mark |
|---|---|---|---|
| The file's checksum | any | `Same` | none |
| Another checksum the link knows, in `base` or `past` | any | `Ahead` | `↑` |
| A checksum the link never knew | unchanged since `base` | `Theirs` | `↓` |
| A checksum the link never knew | changed | `Both` | `!` |
| Nothing | any | `Gone` | `!` |
| Not read yet | any | `Unread` | dimmed address |

A slot that reports no checksum, such as Settings, is compared by length against
`base`.

### Rows and directions

A linked file whose standing is `Theirs`, `Both` or `Gone` is looked at once more
before it becomes a row. If the slot holds a sound under another name than the link
recorded, or holds nothing, drawbar looks for the file's sound in the other slots of
its class:

- in exactly one, which no file is linked to: **moved**, and the link follows;
- in exactly one, whose own file's sound has moved elsewhere: also **moved**, as one
  step of a swap or rotation, which goes whole or not at all;
- otherwise **replaced** when the slot holds another sound, and **removed** when it
  is empty.

| Row | Goes by itself | May be turned to |
|---|---|---|
| Placed by hand | `↑` | nothing else |
| Played by a program going up | `↑`, to a free slot | nothing else |
| Newer here | `↑` | `↓` |
| Newer on the keyboard | `↓` | `↑` |
| Both changed | waits | `↑` or `↓` |
| Removed on the keyboard | waits | `↑` writes it back, or `Unlink` |
| Moved | `↓`, the link follows | nothing else |
| Replaced | `Unlink` | nothing else |

A slot's version is never brought into a piano or sample, whose slot is not read
whole, or into a file with unsaved edits; those rows go `↑` or wait. A file has at
most one row with a direction: a hand-placed send of a file whose own slot changed on
the keyboard waits until that row is settled.

### Sync runs what it showed

Every press reads the keyboard again before the sheet opens. Each list reads in under
0.3 seconds on an Electro 5.

Pressing the sheet's button freezes a plan: each row, its direction, and the versions
of the file, the link and the slot the row was shown with. Sync runs that plan and
nothing else.

- A step runs only while the file and the link are still as shown.
- A write carries what its slot is expected to hold. The worker reads the slot's info
  just before the backup, delete and write, and refuses the write when the slot holds
  anything else. The check sits at the instrument boundary because a panel Store made
  while a host is attached is not announced. Observed once on an Electro 5; rename,
  move and delete while attached are untested.
- A bring-in saves only a body with the checksum its row showed, or the length and
  name for a slot that reports no checksum.
- A step that cannot run is logged, and the sheet opens again on a fresh read.

### Plays

A program names a piano or sample by an id. The id is MurmurHash2 (32-bit, seed 0) of
the name the instrument keeps the object under, reading each four-byte block
big-endian. Confirmed on hardware, on an Electro 5; only ASCII names were observed.
`nord_format::components::library_id` computes it.

So what a program plays, and which programs play a piano, are found by hashing file
names and slot names, with or without a keyboard attached. Nothing is stored. A
bundle unpacked into the library exports again with what its programs play.

## Drawbacks

- The library holds a fact per file per keyboard, and a keyboard with no serial is
  every keyboard of its model. Two Electro 5s kept alike sync through one library. Two
  that hold different sounds each read as many changes, and their owner needs a
  library for each.
- History is eight checksums and no bytes. A slot holding a version older than those
  reads as changed on the keyboard. Turning a `↓` row to `↑`, or picking the file on a
  both-changed row, discards the keyboard's version.
- A file older than its link reads as newer than its slot. That is right for a file
  put back by undo, and wrong for one a sync client has not delivered yet when the
  link another drawbar wrote for it arrives first.
- A change made at the panel while drawbar is attached shows at the next Sync press,
  or as a refused write.
- Every press reads every list. That is quick on an Electro 5 and unmeasured on
  instruments with more slots.
- Plays rests on a hash confirmed on one model with ASCII names.

## Rationale and alternatives

| Option | Why not |
|---|---|
| Link by equal bytes, as at `aef7dc79` | New programs and duplicates link by coincidence |
| Trust the slot in each program's header | Every header names a slot, so New files and copies claim one, and an imported backup claims the user's slots. It gives no direction. It survives as evidence for the automatic link on attach and as the first slot Send to… offers |
| Name each keyboard and ask which is attached | An identity drawbar cannot check |
| Follow a moved slot silently | An emptied slot plus an unrelated identical copy looks the same, so a move is shown in the sheet first |
| Send on Save | Writes to a keyboard that may be on stage |
| Keep every version's bytes, with Restore | The store's trash is trimmed, so it is not a history. A bounded version store is a design of its own |
| Store what each program plays | The name hash derives it, and a stored relation could disagree with the names |
| Bring pianos and samples in from the keyboard | Large reads of objects the panel cannot edit. Copy to this computer does it on request |
| Separate flows: a send review, a sheet on attach, and a pick for each conflict | This was built first. A send could be refused at write time, and the user then had to find the file, read its mark and pick, across three surfaces. One sheet with a direction per row replaced all three |

**Why the sheet freezes a plan.** An earlier build recomputed the rows after the
button was pressed. Review found cases where it then wrote a row the sheet had not
listed, or brought in a sound other than the one shown. Running only what was shown,
each step guarded by the versions it was shown with, removes that class of fault.

**Why a changed name means another sound.** A different program stored over a linked
slot has a checksum the link never knew, the same as a panel edit. Bringing it into
the file would replace the file's sound with an unrelated one. The recorded name
separates the two.

**Why an emptied slot waits.** A sound deleted at the panel on purpose should not
come back because its file still exists.

## Prior art

- Nord Sound Manager transfers a sound one way per action, and backs up and restores
  a whole instrument.
- Version control and file sync tools keep a common base for each file and compare
  both sides against it. `base` and `past` are that base, reduced to what a slot can
  report.
- RFC-0271 expected the library to store which pianos and samples a program plays,
  because the id was thought to be learned only from the instrument or a bundle's
  manifest. The name hash makes that unnecessary.

## Unresolved questions

- Does a set list follow a program moved at the panel, or keep its address? Rewriting
  a set list when its programs land elsewhere waits on this.
- Do panel renames, moves and deletes announce themselves to an attached host?
- Does the name hash hold on other models, and how are names outside ASCII encoded?
- Which other models report a serial?

## Future possibilities

- A bounded store of version bytes, which would give Restore and an undo for a write
  to the keyboard.
- Noticing panel changes while attached, by a read when idle.
- Telling an undelivered file from an edit, by whether the file holds what the
  library last recorded for it.
- Bundle export resolving a set list's programs through links; it uses the slot in
  each program's header.
- Bringing a piano or sample in from the keyboard.
