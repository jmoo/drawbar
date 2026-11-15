# Editing

There is no Apply button. A control you move is set on the document at once,
and the name shows a `*` until you save. Nothing on the instrument changes until
you queue the document and send it.

## The document header

Every document has the same strip across the top: its kind and name, a format
badge, where it lives (the slot it is linked to, a folder, or this computer), its
size, and `edited` or `waiting to send`. The place carries the library's mark:
`↑ Programs 1:7` is ready to send, and `! Programs 7:14` needs attention. Hover
any of them for the detail.

To the right are the two faces, **Revert**, **Export…**, and the document's
main action. It is **Save** while the document is edited; **Send to…** for a
document linked to no slot, which queues it and opens the review to pick its
slot; **Load on instrument** when its slot holds it, which makes the panel play
that slot; and **Send to 1:7** when the slot holds an earlier version or is
empty. A document
whose slot the keyboard changed has **Choose…**, which shows the fields that
differ: **Use the keyboard’s** saves the slot's version into the document, and
**Use mine** queues the document's to the slot. The action is grayed out when
there is nowhere to send to, and becomes a warning such as `Won't fit · 70 MB
over` when the send cannot happen yet. The panel plays what the instrument holds,
so edits you have not sent are not heard.

Saving a linked document that is then newer than its slot queues the send to that
slot.

## The faces

- **Basic** is the sound's controls, in the instrument's own words.
- **Advanced** is the record of the bytes, top to bottom:
  - **About this file**: what the file says about itself. Songs, files
    with nothing to edit and bytes drawbar could not read have none.
  - **Container**: what the file's header states, and whether its checksum
    matches.
  - **On the instrument**, for a document read from a slot: what the
    instrument reports about that slot.
  - The body. For a program, live slot, settings file or preset this is
    **Every field**, a table that includes values the Basic face does not draw.
    **Raw** is the value as last saved. Type into the **Writes** column to set a
    field. A value the field cannot hold is refused, with the reason. A field
    changed since the last save is highlighted and marked with a pencil, and the
    count above the table says how many there are. Samples and pianos list what
    drawbar can edit in them and where each edit lands in the file. A song
    shows its four stored slots, and a file with nothing to edit shows its
    **Body bytes**. Bytes drawbar could not read have no body to show.

A WAV has only Basic, and bytes drawbar could not read have only Advanced, so
neither shows the switch.

## Programs, live slots, settings and presets

These open on a panel divided into sections the way the instrument's is, with a
strip of chips at the top to jump between them. Each field is drawn as the
control the panel uses: a lamp for a switch, a menu for a selector, a knob with
the panel's own reading, drawbars you pull down, a grid for a pattern. A value
drawbar has no name for reads `unrecognized value (6)` with a warning mark, and
stays in its menu so you can change back to it.

Every control answers the keyboard. Tab reaches it, the arrow keys step a knob,
Page Up and Page Down jump it, Home and End take it to its stops, and Space flips
a lamp. Double-click a knob to type a value.

Where a program refers to a piano or sample, drawbar shows its name when that
piano or sample is on this computer or on the instrument. With an instrument
connected, the piano
**Model** lists its pianos of the chosen **Type**. Changing either one points the
program at the piano now chosen, which is what the instrument plays. Sections the
program stores but is not using are folded away.

A control with a morph shows three dots. Pick a morph source in the inspector's
**Morph** card and every morphed control shows that source's target instead.
Editing then writes the morph. **Panel**, or **Back to the panel** above
the sections, shows the panel's own values again.

## Set lists

A song from a set list opens as the programs it plays, one row each, with its bank and slot,
the program's name where drawbar knows it, and whether the entry resolves. Drag
rows to reorder them.

## Notes

A text file opens as a box you type in: the set list, the cues, what the desk
needs. A `.txt`, a `.md` or any other plain text file of up to 256 KiB opens
this way, and **New ▸ Text note** starts an empty one. Tab types a tab, and
pasted control characters other than tabs and line breaks are dropped.

A note is saved, reverted, renamed, filed, tagged and exported like anything
else on the list. The header counts its lines where another document's header
gives its size. Notes stay on this computer; see
[What is supported](../getting-started/support.md).

## Files with nothing to edit

A file drawbar recognizes but cannot yet edit opens with what the container
says, and its bytes are under **Body bytes** on the Advanced face. It can still
be sent, copied and tagged, and goes back to the instrument byte for byte as it
came off.

Sample instruments and piano libraries have editors of their own. See
[Samples](samples.md) and [Pianos](pianos.md).
