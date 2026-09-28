# Your files

**This computer** is drawbar's own list: what you open, what you make, and what
you copy off an instrument.

## Opening and making

Drop files on the window, or use **File ▸ Open…**. Every file is decoded and
immediately re-encoded to check that its bytes come back identical, and the
activity log tells you if one does not. A file drawbar cannot read still gets a
row, so you can see what went wrong.

**New** makes a fresh program, live slot, set list, settings file or preset for
the instruments drawbar supports, a sample instrument or piano library from WAVs,
a text note, a Sample Editor project, or a folder. A line across the menu
separates what an instrument can hold from what only this computer keeps. A fresh
Stage file has every control at zero. It is not a factory program.

## Views

Opening a slot on the instrument shows a **view**: the instrument's own copy, in
place. It is not on this computer until you click **Keep on this computer**. An
edited view is kept when its tab closes, so the edits are not lost.

## Saving and reverting

Every edit lands at once, and the name turns italic with a `*` until you save or
revert. **Save** (⌘S) writes the file. If the sound belongs to a slot on the
connected instrument, Save also queues it for sending. **Revert** goes back to
the last save, and is the only undo. An edit you have not saved is kept too, and
comes back unsaved the next time drawbar starts.

## Where it lives

On the desktop, this computer is a folder of real files: `drawbar` in your Music
folder. That is `~/Music/drawbar` on macOS and `Music\drawbar` in your user folder
on Windows. On Linux it is in the music folder `xdg-user-dirs` names, or
`~/drawbar` where there is none. drawbar makes the folder the first time it keeps
something there. Hover **This computer** for its path, or right-click it and
choose **Show the library folder**.

Every sound is a file there, named as the browser shows it plus its extension,
and every folder in the browser is a folder there. Finder, your backups and Nord
Sample Editor see what drawbar sees. A hidden `.drawbar` folder beside them keeps
what a file cannot: tags, the slot a sound came from, and edits not yet saved.
drawbar makes it the first time it changes something in the folder.

The browser shows the files drawbar opens: Nord files, Sample Editor projects,
notes, MIDI files and SysEx dumps. **Show all files**, in the View menu or on
This computer's menu, lists the rest by name as well. drawbar lists at most
10,000 files and folders in a library, and reads at most 1 GiB of files from it.
Past either limit it says so: a folder it did not list in full, or could not
read, shows **not all listed**, and a file it did not read is marked **not
read**.

**Delete…** on a sound deletes its file. **Remove folder** moves what was in the
folder up a level and deletes nothing.

Only one drawbar at a time changes a library. A second one opens it read-only
and says so, and so does a drawbar older than the one that last wrote it.

In the browser, this computer is kept in the browser's own storage, which holds
about 2 MB in all. The log says when something does not fit. Samples and piano
libraries are usually larger, so export them.

This version of drawbar starts with an empty list. What an earlier version kept
is not carried over, and drawbar says so once. Long-term storage is not
guaranteed while drawbar is in alpha. Keep your own copies.

## Opening another folder

On the desktop, **File ▸ Open library folder…** opens any folder as the library:
a Sample Editor project folder, a sample pack, a folder on a shared drive. A
window has one library open at a time, and the browser calls it by its folder's
name where it would say This computer. **File ▸ Open recent library** switches
between the libraries opened lately, and always lists your own. The same items
are on This computer's menu. drawbar opens the last library again when it
starts.

Switching writes the library you leave first. Edits you have not saved stay in
its `.drawbar` folder and come back, still unsaved, when you open it again.
Views of the instrument's slots stay open, and sounds of the library you leave
come off the send queue.

drawbar changes nothing in a folder you open until you change something there,
and only then makes its `.drawbar` folder. A folder drawbar cannot write, or one
another drawbar already has open, opens read-only; hover its name for the
reason. Two files whose names differ only in case both show, marked, and drawbar
renames neither.

In the browser, the library is the browser's own, and no other folder opens yet.

## Changes made outside drawbar

drawbar looks at the folder again whenever its window comes back to the front,
and before it sends anything to the instrument.

- A sound renamed or moved outside drawbar keeps its tags.
- A file changed outside drawbar is shown as it is now. If you had unsaved edits
  to it, drawbar asks: **Keep mine** (your next save writes over the file),
  **Take theirs**, or **Keep both** (yours becomes a new file beside it).
- A file deleted outside drawbar leaves the list, unless it had tags, unsaved
  edits, or a slot it came from. Then it stays, marked missing, until you save it
  back or delete it.
- If a sound waiting to be sent changed on disk, nothing is sent until you have
  looked at it.

## Exporting and names

**Export…** writes a copy of the file: a download in the browser, a save dialog
on the desktop.

A sound's name comes from the slot it was read from, the filename, or the New
menu, and drawbar shows it without the extension. Rename with F2 or the name box
in the document header. Where the file stores a name of its own, as samples and
pianos do, that box edits the stored name.

On this computer a name is a filename that macOS, Windows and Linux can all hold:
no `/ \ : * ? " < > |`, no control characters, no space at either end, no dot at
the start or end, and not a name Windows keeps for a device, such as `CON` or
`LPT1`. drawbar refuses such a name when you type it. Two names in one folder may
not differ only in case. When a name you chose is taken, drawbar asks whether to
**Overwrite** what is there, which keeps its tags, or **Keep both** under a
numbered name. A folder that already holds two names differing only in case,
which some disks allow, shows both and marks them. drawbar renames neither.
