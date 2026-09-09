# Files on this computer

Files arrive by drag-and-drop or **Open…**, and leave through **Export…** on a
row's context menu or the document header. **New** makes a fresh program, live
or settings document.

Every file is decoded and immediately re-encoded to check the bytes come back
identical. A file that fails to decode still gets a row, because reporting a bad
file is the point of opening it.

**The list is kept between sessions** — in the browser's own storage, or a small
file beside the app natively — and is re-decoded and re-checked on the way back
in. Edits are kept without being asked for. Two limits are said out loud when
they bite: a single sound over about a megabyte is not kept, and the whole list
is capped well under the browser's quota.

A tab holds a working copy. **Revert** goes back to the bytes the tab opened
with — the only undo there is — and **Export…** writes the file.

A name is this app's own: a Nord file stores none. It is set when the bytes
arrive and is kept **verbatim** everywhere after that. Only the filename an
**Export…** suggests is made path-safe, because that is the one place a name
meets a filesystem.

More detail in
[`crates/drawbar/README.md`](https://github.com/jmoo/drawbar/blob/master/crates/drawbar/README.md).
