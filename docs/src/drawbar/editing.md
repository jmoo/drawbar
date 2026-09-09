# Editing

Looking at a document and changing it are the same thing. There is no Apply: a
control you move is set on the working copy that instant, the bytes are
re-encoded and re-checked, and the tab picks up a dot. Every document has two
faces.

## Basic

The front panel, laid out like one: **knobs** for the values that sweep,
**lamps** for the switches, and titled sections — Keyboard & split, Organ, Piano,
Sample, Effects, EQ — none of which folds away. Only the engines a part is
actually playing are shown, so a document shows what the keyboard would be
showing.

**Drawbars** are the widget the crate is named after: pull down to draw out, with
the positions in digits underneath (`88 8800 000`). No hex anywhere in Basic.

**Every control answers the keyboard.** Tab reaches them; a knob steps on the
arrows and goes to its stops on Home/End, and a lamp switches on Space or Enter.

Sample instrument zone edits cover the decoded Sample Library 2.0, `.nsmp3` and
`.nsmp4` layouts; earlier `.nsmp` zone layouts remain read-only, and audio stays
verbatim.

## Advanced

The other face of every document, and the only face for something with no
friendly view. It is the whole body as a table: one row per field the library
declares — label, path, bit placement, decoded value, and the stored spelling in
a cell you can type into. This is the engineer's view.

Each control and its behaviour is described in
[`crates/drawbar/README.md`](https://github.com/jmoo/drawbar/blob/master/crates/drawbar/README.md).
