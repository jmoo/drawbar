# Files on this computer

Files arrive by drag-and-drop or **Open…**, and leave through **Export…** on a
row's context menu or the document header. Every file is decoded and immediately
re-encoded to check the bytes come back identical; a file that fails to decode
still gets a row, because reporting a bad file is the point of opening it.
**New** makes a fresh program, live or settings document.

**The list is kept between sessions.** What is on this computer is written to the
browser's own storage (or a small file beside the app natively) and comes back on
the next start, re-decoded and re-checked on the way in. Edits are kept without
being asked for. Two limits, both said out loud when they bite: a single sound
over about a megabyte is not kept — a sample runs to megabytes and one would fill
the store — and the whole list is capped well under the browser's quota. Removing
a row removes it from the store.

## The document

A tab holds a working copy. Its header says where it came from; **Revert** goes back to
the bytes the tab opened with — the only undo there is — and **Export…** writes the file.

## Names

A name is this app's own: a Nord file stores none. It is set when the bytes arrive —
from the slot the instrument read it out of, from the filename, or from the New menu —
and nothing after that re-derives it. It is kept **verbatim**: "Big strings" is
"Big strings" in the list, in the tab, and on the slot a send names. Only the
filename an **Export…** suggests is made path-safe, because that is the one place a
name meets a filesystem.

What a control does to the working copy is covered in [Editing](editing.md), and
what happens to a document copied off an instrument in
[The instrument](instrument.md).
