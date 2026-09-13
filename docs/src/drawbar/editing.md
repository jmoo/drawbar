# Editing

Looking at a document and changing it are the same thing. There is no Apply: a
control you move is set on the document at once, the bytes are re-encoded and
checked, and the document reads as edited. Nothing on the instrument changes until
you queue the document and send it.

## The document header

Every document has the same strip across its top. From left to right:

| Part | What it shows |
|---|---|
| Kind and name | The kind's glyph, then the name. The name is a box to type in, or plain text for the instrument's Settings, which are never renamed. A piano library's name is two boxes, `Name` `#` `Variant`. |
| Format badge | `program v3`, `nsmp v2`, `set list`, `project` and so on. Hover it for the content or stream version. |
| Where | A slot, such as `Samples 1:12`; the folder a file came from, such as `~/Nord/projects`; or `This computer`. |
| Size | The file's size. A set list counts its entries, and Settings show none. |
| State | A dot and a phrase: `edited`, `matches keyboard`, `differs from keyboard`, `waiting to send` or `on the keyboard`. Hover it for the whole sentence. |
| Faces | **Edit**, **Metadata** and **Advanced**, as one control. A document shows only the faces it has. |
| **Revert**, **Export…** | Back to the last saved bytes, and a copy of the file. |
| The action | The one thing this document is for. |

The action is usually **Queue send**, which adds the document to the send queue for
its slot. It is dashed, and does nothing, when there is nothing to send to: no
instrument attached, or a document that stands on no slot. It turns to a warning
with the reason when the send cannot happen yet, such as `Won't fit · 70 MB over`.

Under the name, some kinds add a row: a program's tags, or a sample's category or
sub name.

The header folds as the window narrows. At 1000 px the quiet actions lose their
words, at 860 px the faces lose theirs, and at 720 px the action keeps only a short
label. Hover a control for what it does.

## The three faces

- **Edit** holds the controls that change the sound, in the instrument's own words.
  A document with nothing drawbar can draw has no Edit face.
- **Metadata** is what the file says about itself, and changes nothing. It shows the
  container and its check, the bytes that changed since the last save, and the raw
  decode behind **Show the decode**. For a document from a slot, **Read slot
  details** asks the instrument for the slot's own record and dependency list.
- **Advanced** is the engineering view: every field, capabilities, offsets and raw
  values.

Each document remembers the face it was left on.

## Programs, live, settings and presets

A body with a field registry opens as a field document. That covers:

- Electro 5 programs, Live slots and Settings;
- Stage 2, 3 and 4 programs and live slots;
- Stage 3 and 4 synth presets;
- Stage 4 organ and piano presets.

Stage 2, 3 and 4 programs and presets are supported, but have not been tested on
real instruments.

### Sections

The document is divided into sections the way the panel is. A strip of chips
across the top names each section with its field count; click one to jump to it.

A group the file stores but does not use in the state it holds is hidden, not
cleared. The section ends with one line naming what is stored and idle, with a
link to **Advanced**. Where the panel chooses between two alternatives, both are
drawn side by side: the one playing is marked **playing**, and **select** switches
to the other.

### Controls

Each field is drawn as the control the panel uses:

- a **switch** is a lamp with its word;
- a **selector** is a menu. A value drawbar has no name for reads `unknown (6)` and
  is flagged, and it stays in the menu so you can change back to it;
- a **knob** shows the panel's reading where the unit is known;
- a **bipolar** control marks its centre and reads as an offset from it;
- a **shift** is a stepper;
- **drawbars** are pulled down to draw them out;
- a **pattern** is a grid of steps;
- a **reference** to a piano or sample shows the instrument's name for it, or the
  id the file stores.

A label drawbar has no name for shows the field's path instead, with a tag glyph.

**Every control answers the keyboard.** Tab reaches it. A knob steps on the arrow
keys, jumps ten on Page Up and Page Down, and goes to its stops on Home and End. A
lamp switches on Space or Enter. Drag a knob up to turn it up, or double-click it to
type a value; what you type is clamped to the knob's stops.

**Transpose is one control** on the Electro 5, a lamp and a number together. Moving
the number lights the lamp, as the panel's button does.

**The piano a program plays** is stored as an id, and only the instrument knows its
name. With an instrument attached, a document opened from a slot reads the
program's dependencies and shows the name. **Ask again** reads them again, and **Ask
the instrument** is there when a read found nothing. Once the Pianos folder has been
read, the Model dial lists the instrument's pianos by position. Where that list and
the dependency reply disagree, the document says so and trusts the instrument.

### Morph

A control with a morph stored shows three dots under it. Where a body has morph
slots, the chip strip ends with **MORPH**: **Panel**, **Wheel**, **Aftertouch**
and **Control pedal**, each slot with a count of the targets stored under it. Pick
one and every morphed control shows that slot's stored target instead of the panel
value. A banner says so, and editing then writes the morph slot. **Back to the
panel** returns.

### Pending and sending

Fields whose value differs from the last save are pending. The header reads
`3 pending`, and its action reads **Queue send · 3**. The pending sets are applied
as one batch, all or none.

### Advanced

**About this file** lists the format, the field count, how the sections were
decided, where the document is stored, and which instrument the format belongs to.

**Every field** is the whole body as a table, in registry order: **Path**,
**Bits**, **Control**, **Raw** and **Writes · editable**. Nothing is hidden,
including reserved bits and fields the Edit face does not draw. Type into the
**Writes** column to set a field; the value is taken as spelled, and a value the
field cannot hold is refused with the reason. **Filter** narrows the rows by path
or name.

## Set lists

An Electro 5 set list opens as **The four programs this set list plays**. Each row
has a drag handle, its number, the program's name, its **Bank : slot**, its
**State**, and an arrow that opens the program.

- The name comes from the attached instrument's scan or a sound on this computer
  for that slot. Otherwise it reads `unresolved` or `not read yet`.
- Type a bank and slot as the panel numbers them, from 1.
- **State** reads `resolves`, `no program at 7:4`, `needs a piano` or `not read yet`.
  There is no empty entry: the format has no way to say none, so every entry names a
  slot.
- Drag a row to reorder the list. Reordering rewrites every entry below the move.

The header's state sums the entries: `every entry resolves`, or how many need
attention. **Advanced** shows **Slots as stored**, the four addresses as the body
holds them.

Stage set lists are not decoded, and have no set list page.

## Files drawbar has no fields for

A body with no field registry, such as a Lead performance, an Electro 6 program or
a bundle, opens as **Nothing to edit here yet**. The page lists what the container
says, explains why nothing is drawn, offers **Save a copy…**, and shows the first
192 bytes of the body. **Advanced** shows the whole body. The header's action reads
**Send as-is**: the file can still be sent, copied, tagged and placed, and goes up
byte for byte as it came down.

A file that did not decode has only its Metadata face.

Sample instruments, Sample Editor projects and piano libraries have editors of their
own: see [Samples](samples.md) and [Pianos](pianos.md).
