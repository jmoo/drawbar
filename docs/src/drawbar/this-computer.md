# Files on this computer

**This computer** is drawbar's own list of sounds. It holds what you open, what you
make, and what you copy off an instrument.

## Opening files

Drop files anywhere on the window, or use **File ▸ Open…** (⌘O) or the toolbar's
open button. Each file is decoded and at once re-encoded, to check that its bytes
come back identical. The activity log says when a file does not re-save byte for
byte. A file that does not decode still gets a row, so you can see what was wrong
with it.

A dropped WAV opens as a document that can be encoded into a sample instrument;
see [Samples](samples.md#from-a-wav).

## Making something new

The **New** menu is the same in **File**, the toolbar, the tab strip's **+** and the
**This computer** row's menu:

| Item | Makes |
|---|---|
| **Electro 5 ▸** **Program**, **Live slot**, **Set list**, **Settings** | a fresh Electro 5 file |
| **Stage 2 ▸** **Program** | a Stage 2 program |
| **Stage 3 ▸** **Program**, **Synth preset** | a Stage 3 file |
| **Stage 4 ▸** **Program**, **Organ preset**, **Piano preset**, **Synth preset** | a Stage 4 file |
| **Sample Editor project…** | an `.nsmpproj` from WAVs you pick |
| **Sample instrument…** | an `.nsmp` encoded from WAVs you pick |
| **Piano library…** | an `.npno` built from WAVs you pick |
| **New folder** | a folder on this computer |

An Electro 5 program comes from the library's own constructor. The Stage kinds are
every control at zero: they decode and re-save byte for byte, but they are not
factory programs. The three items that take WAVs are covered in
[Samples](samples.md#new-from-wavs) and [Pianos](pianos.md#a-new-piano-library).

## Views and kept sounds

Opening a slot of the instrument opens a **view**: the instrument's own copy, shown
in place. It is not on this computer, and its document says so: *Viewing Programs
7:4 on the instrument.* **Keep on this computer** puts it in the list.

An unedited view goes away when its tab closes. A view with edits is kept on this
computer when its tab closes, because it is the only copy of those edits.

## Saving and reverting

Every edit lands on the document at once. The name then turns italic with a `*`,
until you save or revert.

- **Save** (⌘S, **File ▸ Save**, or the toolbar's save button) marks what a kept
  sound holds now as saved. If the sound stands for a slot on the attached
  instrument, Save also queues it for that slot. For a view, Save writes back to the
  slot at once.
- **Revert** in the document header, or **File ▸ Revert to saved**, goes back to
  the bytes last saved. It is the only undo.

## What is kept between sessions

The list is written to the browser's own storage, or natively to a file beside the
app, and comes back on the next start. Every sound is decoded and checked again on
the way in. Saved bytes and unsaved edits are both kept, as are folders, tags, the
theme and the dock layout.

The store has limits, and the activity log says when one applies:

- A single sound over about a megabyte is not kept. Samples and piano libraries
  often run larger than that, so export one if you want it after the tab or window
  closes.
- The whole list is capped at about 3 MB, below the browser's own quota. What does
  not fit is left out.
- Long-term storage is not guaranteed while drawbar is in alpha. Back up your files
  elsewhere.

**Remove from list** in a row's menu takes a sound off the list and out of the
store.

## Exporting

**Export…** (⇧⌘E), in the document header, the File menu or a row's menu, writes a
copy of the file. A browser downloads it; the desktop app asks where to save it,
writes beside that path, and renames over it only once the write has finished.

## Names

A name is drawbar's own, and a Nord file stores none. It is set when the bytes
arrive: from the slot they were read from, from the filename, or from the New
menu. Nothing re-derives it after that. The interface shows it without its format
extension; the log, rename and the export filename keep the name in full. Only the
filename that **Export…** suggests is made path-safe: "Big strings" is exported as
`Big-strings.ne5p`.

Rename a sound with **F2**, **Rename** in its menu, or the name box in its document
header. Where the file stores a name of its own, as a sample or piano library does,
the header's box edits that stored name instead; see
[Editing](editing.md#the-document-header).

What a control does to a document is covered in [Editing](editing.md), and what
happens to a sound bound for an instrument in [The instrument](instrument.md).
