# Editing

There is no Apply. A control you move is set on the document at once, and the
name shows a `*` until you save. Nothing on the instrument changes until you
queue the document and send it.

## The document header

Every document has the same strip across the top: its kind and name, a format
badge, where it lives (a slot, a folder, or this computer), its size, and its
state: `edited`, `matches keyboard`, `differs from keyboard`, `waiting to send`
or `on the keyboard`. Hover any of them for the detail.

To the right are the document's faces, **Revert**, **Export…**, and the one
action the document is for. That is usually **Queue send**. It is greyed when
there is nowhere to send to, and becomes a warning such as `Won't fit · 70 MB
over` when the send cannot happen yet.

## The faces

- **Edit** is the sound's controls, in the instrument's own words.
- **Metadata** is what the file says about itself, and changes nothing.
- **Advanced** is every field in the file as a table, for when you need a value
  the Edit face does not draw. Type into the **Writes** column to set one. A
  value the field cannot hold is refused, with the reason.

## Programs, live slots, settings and presets

These open on a panel divided into sections the way the instrument's is, with a
strip of chips at the top to jump between them. Each field is drawn as the
control the panel uses: a lamp for a switch, a menu for a selector, a knob with
the panel's own reading, drawbars you pull down, a grid for a pattern. A value
drawbar has no name for reads `unknown (6)`, and stays in its menu so you can
change back.

Every control answers the keyboard. Tab reaches it, the arrow keys step a knob,
Page Up and Page Down jump it, Home and End take it to its stops, and Space flips
a lamp. Double-click a knob to type a value.

Where a program refers to a piano or sample by id, drawbar asks the connected
instrument for its name and shows it. Sections the program stores but is not
using are folded away, with a line saying so.

A control with a morph shows three dots. Pick a morph source from the **MORPH**
chips and every morphed control shows that source's target instead. Editing then
writes the morph.

## Set lists

A set list opens as the programs it plays, one row each, with its bank and slot,
the program's name where drawbar knows it, and whether the entry resolves. Drag
rows to reorder them.

## Files with nothing to edit

A file drawbar recognises but cannot yet edit opens with what the container says
and a look at its bytes. It can still be sent, copied and tagged, and goes up
byte for byte as it came down.

Sample instruments and piano libraries have editors of their own. See
[Samples](samples.md) and [Pianos](pianos.md).
