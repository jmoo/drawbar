# Editing

Looking at a document and changing it are the same thing. There is no Apply: a control
you move is set on the working copy that instant, the bytes are re-encoded and
re-checked, and the tab picks up a dot. Every document has two faces.

## Basic

**Basic** is the front panel, and it is laid out like one: **knobs** for the values that
sweep, **lamps** for the switches, names printed *under* what they name, and controls
running left to right in strips that wrap rather than down a column of form rows.
Sections are titled boxes — Keyboard & split, then Organ, Piano, Sample, Effects, EQ —
and none of them folds away, because a control you cannot see is a control you do not
know you have. **Only the engines a part is actually playing
are there**: set both parts to piano and the organ section goes, and the part pickers
that bring it back are in Keyboard & split, which leads because it never goes.

**A document shows what the keyboard would be showing.**

- The organ section carries the model picker and **only the selected model's**
  registrations: two nine-drawbar presets for the B3 with its vibrato and percussion,
  the Vox's bars and vibrato, the Farfisa's registers (which the instrument reads as
  on/off tabs at 5 and above), the pipe organ's bars and nothing else. The preset the
  instrument is playing is marked, and clicking the other one switches it.
- In **b3+bass**, preset 1 is the bass manual: two drawbars, in their own fields. The
  nine nibbles they shadow hold stale leftovers and are not shown at all.
- **Transpose is one control**, a lamp and a number written together, because that is
  what the panel's button is. The instrument ignores the amount while the lamp is dark,
  and moving the number lights it.
- **The piano section names the piano** where the instrument can name it. A program
  stores its piano as an id and the panel's own dial position, and no file anywhere
  carries the name — so with an instrument attached the document reads the program's
  dependencies as it opens and the name appears on its own; *Ask again* re-reads it, and
  *Ask the instrument* is there for a read that found nothing. Without an instrument the
  id is shown and says as much.
- **The Model dial is a list of pianos** once the instrument's Pianos folder has been
  read: the scanned names of the document's category, each at its dial position. Where
  the dependency reply and the list disagree the document says so and trusts the
  instrument — a stored category and model are slot coordinates, and go stale when the
  library is reorganized. Unscanned or unattached, the dial stays a number.
- A picker offers only values the library can name. A file holding one it cannot reads
  as *unrecognized value (6)* and keeps it in the list, so changing away from it can be
  undone.
- Reserved bits, unexplained fields and the library ids that name a program's piano and
  sample are not offered here. They are all in Advanced.

**Drawbars** are the widget the crate is named after: pull down to draw out, with the
positions in digits underneath (`88 8800 000`). No hex anywhere in Basic.

**Every control answers the keyboard.** Tab reaches them; a knob steps on the arrows,
jumps ten on Page Up/Down and goes to its stops on Home/End; a lamp switches on Space or
Enter. A knob is dragged up to open it out, and double-clicking one opens its number for
typing — what is typed is clamped to the stops, because a knob cannot be turned past
them.

Sample instruments show their name and the stretch of keyboard each zone covers, with
root key and top note as note names (`C4` is middle C). Zone edits cover the decoded
Sample Library 2.0, `.nsmp3` and `.nsmp4` layouts; earlier `.nsmp` zone layouts remain
read-only. Audio stays verbatim. A zone is decoded on request, drawn as a waveform and
played; encoded audio can be large, so nothing is decoded until asked.

Set lists are their four program slots, each editable as the `BANK:SLOT` pair the
instrument shows. Sample Editor projects (`.nsmpproj`) show the instrument's name,
each zone's root key and key range, and the audio files the editor will look for —
all editable, saved back as the same text file the editor reads.

## Advanced

The other face of every document, and the only face for something with no friendly view
(a file that did not decode).

It is the whole body as a table: one row per field the library declares — label, path,
bit placement, decoded value, and the stored spelling in a cell you can type into.
Nothing is hidden and nothing is prettied up. Reserved bits, both halves of the
transpose pair, the library ids and the wide hex registers are all here, in declaration
order, with a filter box over the names. A value the library refuses says so beside the
cell and leaves what you typed where it is. `unknown (9)` is a legal spelling here —
this is the engineer's view.

Under the table sits the record: the verify badge, the CBIN header numbers, the bytes
that moved since the tab opened (with the checksum rows annotated so they do not read as
a second edit nobody made), the `{:#?}` dump, and — for something read off the
instrument — the slot's own info and dependency list, on request.
