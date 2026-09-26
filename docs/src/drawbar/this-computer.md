# Your files

**This computer** is drawbar's own list: what you open, what you make, and what
you copy off an instrument.

## Opening and making

Drop files on the window, or use **File ▸ Open…**. Every file is decoded and
immediately re-encoded to check that its bytes come back identical, and the
activity log tells you if one does not. A file drawbar cannot read still gets a
row, so you can see what went wrong.

**New** makes a fresh program, live slot, set list, settings file or preset for
each supported instrument, a sample instrument or piano library from WAVs, a
text note, a Sample Editor project, or a folder. The rule across the menu parts
what an instrument holds from what only this computer keeps. A fresh Stage file
has every control at zero. It is not a factory program.

## Demo sounds

**Get the demo sounds** on the welcome sheet (**Help ▸ Welcome**) fetches a tine
electric piano and a looped pad from drawbar.app into a **Demo sounds** folder.
The pad comes in two sample formats: the `.nsmp` plays on an Electro 5. Asking
again brings back any you have removed and leaves the rest alone.

## Views

Opening a slot on the instrument shows a **view**: the instrument's own copy, in
place. It is not on this computer until you click **Keep on this computer**. If
you edit a view, it is kept when its tab closes, so the edits are not lost.

## Saving and reverting

Every edit lands at once, and the name turns italic with a `*` until you save or
revert. **Save** (⌘S) marks the current state as saved. If the sound belongs to a
slot on the connected instrument, Save also queues it for sending. **Revert** goes
back to the last save, and is the only undo.

## What is kept

The list, with its folders, tags, edits and layout, is stored in the browser, or
beside the desktop app, and comes back next time. Two limits apply, and the log
says when they bite: a single sound over about a megabyte is not kept, and the
whole list is capped at about 3 MB. Samples and piano libraries are usually
larger, so export them.

Long-term storage is not guaranteed while drawbar is in alpha. Keep your own
copies.

## Exporting and names

**Export…** writes a copy of the file: a download in the browser, a save dialog
on the desktop.

A sound's name comes from the slot it was read from, the filename, or the New
menu, and drawbar shows it without the extension. Rename with F2 or the name box
in the document header. Where the file stores a name of its own, as samples and
pianos do, that box edits the stored name.
