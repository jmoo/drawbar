# Your files

**This computer** is drawbar's own list: what you open, what you make, and what
you copy off an instrument.

## Opening and making

Drop files on the window, or use **File ▸ Open…**, and drawbar copies them into
the library; the files you chose stay where they were. A file dropped on a folder
in the tree goes into that folder where the browser or the system says where it
was dropped, and into the top level otherwise. A library drawbar cannot write
holds what you open in memory instead. Every file is decoded and immediately
re-encoded to check that its bytes come back identical, and the activity log
tells you if one does not. A file drawbar cannot read still gets a row, so you
can see what went wrong.

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
comes back unsaved the next time drawbar starts. An edit of a sample instrument
or piano library is the exception: it is kept only until you quit or open
another library, and drawbar asks before opening another discards it.

## Where it lives

On the desktop, this computer is a folder of real files in drawbar's own data:
`~/Library/Application Support/drawbar/library` on macOS,
`~/.local/share/drawbar/library` on Linux (under `$XDG_DATA_HOME` where that is
set), and `%LOCALAPPDATA%\drawbar\library` on Windows. drawbar makes the folder
the first time it keeps something there. Hover **This computer** for its path, or
right-click it and choose **Show the library folder**.

Every sound is a file there, named as the browser shows it plus its extension,
and every folder in the browser is a folder there. Finder, your backups and Nord
Sample Editor see what drawbar sees. A hidden `.drawbar` folder beside them keeps
what a file cannot: tags, the slot a sound came from, and edits not yet saved.
drawbar makes it the first time it changes something in the folder.

The browser shows the files drawbar opens: Nord files, Sample Editor projects,
notes, MIDI files and SysEx dumps. **Show all files**, in the View menu or on
This computer's menu, lists the rest by name as well. The number beside a folder
counts the files drawbar opens in it and in every folder inside it. Beside This
computer it counts the whole library.

A large library comes in a part at a time, the top of the tree first, and the
status bar counts the files as they arrive. drawbar lists each file by its name,
size and date without opening it, so searching, the kinds in the tree and the
counts beside folders work while the rest is still arriving. A file is read when
you need it: when its row is in view, when you select or open it, or when you
send, export, copy or move it. Until then its row says **reading…** and its kind
comes from its extension. Whether it matches a slot on the instrument by its
contents, and the library a program needs, show once it is read. Once the listing
is done, drawbar reads each file with tags or a slot it came from in the
background, so those match their slots before you look at them.

drawbar remembers what it read. For each file it keeps a small summary: its kind,
the instrument it is for, the checksum it matches a slot by, the library a program
plays, and the WAVs a Sample Editor project names. The summaries live in drawbar's
own data, never in the library: in `library-cache.ron`, in
`~/Library/Application Support/drawbar` on macOS, `~/.local/share/drawbar` on
Linux and `%APPDATA%\drawbar\data` on Windows, and in the site's IndexedDB in
the browser. On macOS and Linux that is the folder the library is in. When you open the library again, a file whose size and date have not
changed shows its kind and matches its slot without being read, and is read only
when you select, open or act on it. Deleting the summaries loses nothing: drawbar
reads the files again as it needs them.

You can rename, move and make folders while a library is still being listed. A
folder you remove waits until everything in it has been listed.

drawbar lists at most a million files and folders in a library, and holds at most
1 GiB of its files in memory. A folder it did not list in full, or could not
read, shows **not all listed**. A file it could not read is marked **not read**.

When reading a file would go past the 1 GiB, drawbar lets go of the files you
used least recently, those with tags or a slot they came from last, and reads one
again when you need it. It keeps every file that is open, selected, in view,
unsaved or waiting to be sent. A file that would go past the 1 GiB with only those
held is marked **not read** until there is room for it again.

Piano libraries and sample instruments run to hundreds of megabytes, so they
stay in their files: drawbar holds in memory only the part it shows or plays,
and reads one whole when you send it or copy it. Saving an edit of one writes
its file again from itself, and exporting or sending one saves it first. When one is read, its checksum is checked in the background. Its row says
**checking…** until that is done, and **failed verification** if the file does
not match its checksum. Such a file is not sent. What stays in its file does
not count toward the 1 GiB.

**Delete…** on a sound deletes its file. **Remove folder** moves what was in the
folder up a level and deletes nothing.

Only one drawbar at a time changes a library. A second one, or a second browser
tab, opens it read-only and says so, and so does a drawbar older than the one
that last wrote it.

In the browser, this computer is the same kind of folder, kept in the storage
the browser gives drawbar.app and nothing else can see. Samples and piano
libraries fit; the limit is what the browser allows the site, and **Help ▸ About
drawbar** shows how much is used. The first time drawbar writes there, it asks
the browser to keep the files even when space runs low. Firefox asks you; other
browsers decide for themselves. Clearing the site's data deletes the library, so
export what you want to keep outside the browser. If the browser gives drawbar
no storage, as some private windows do, drawbar says so, and what you make lasts
until the tab closes.

This version of drawbar starts with an empty list. What an earlier version kept
is not carried over. Long-term storage is not guaranteed while drawbar is in
alpha. Keep your own copies.

## Opening another folder

On the desktop, and in Chrome and Edge, **File ▸ Open library folder…** opens
any folder as the library: a Sample Editor project folder, a sample pack, a
folder on a shared drive. A window has one library open at a time, and the
browser calls it by its folder's name where it would say This computer.
**File ▸ Open recent library** switches between the libraries opened lately, and
always lists your own. The same items are on This computer's menu. drawbar opens
the last library again when it starts.

Switching writes the library you leave first. Edits you have not saved stay in
its `.drawbar` folder and come back, still unsaved, when you open it again.
Views of the instrument's slots stay open, and sounds of the library you leave
come off the send queue. A read-only library cannot keep what is unsaved in it,
so leaving one asks before discarding that.

drawbar changes nothing in a folder you open until you change something there,
and only then makes its `.drawbar` folder. A library another drawbar already has
open, or one whose unsaved edits drawbar cannot read back, opens read-only. A
folder drawbar cannot write turns read-only when drawbar first tries to change
it. Hover its name for the reason. Two files whose names differ only in case
both show, marked, and drawbar renames neither.

In Chrome and Edge, the browser asks whether drawbar may change the folder you
pick. It forgets that answer when you close drawbar.app, unless you told it to
allow the site on every visit. drawbar then opens the browser's own library,
and **File ▸ Reconnect** followed by the folder's name opens yours again once
you allow it. If you do not, the library that was open stays open. drawbar.app
cannot show the folder in your file manager, and the browser's storage limits do
not apply to it. Firefox and Safari cannot open a folder, so there the library is
always the browser's own. Brave turns folder access off, so there **Open library
folder…** is grayed out until you turn on `brave://flags/#file-system-access-api`
and relaunch Brave.

A second tab opens a folder read-only while the first has it open, but a
browser tab cannot tell that the desktop app has the same folder open. Open a
folder in one of them at a time. Two tabs that pick the same folder at the same
moment can also both open it to write.

## Changes made outside drawbar

drawbar looks at the folder again whenever its window comes back to the front,
and before it sends anything to the instrument. While a library is still being
listed, it looks again only at the files it has read, and at the whole folder
once the listing is done. A file whose size and modification time are what
drawbar last saw is taken to be unchanged.

- A sound renamed or moved outside drawbar keeps its tags. drawbar recognizes it
  by its contents, which it reads in the background for each file with tags,
  unsaved edits or a slot it came from. A file renamed before that read, changed
  as well as renamed, or holding the same contents as another shows up as a new
  file, and the old one stays, marked missing.
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
