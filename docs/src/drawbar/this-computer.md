# Your files

**This computer** is your library: the sounds you open, make, or copy off an
instrument. Each one is a file. On the desktop, the files are in a folder you
can see.

> **drawbar is alpha.** Keep your own backups of anything that matters. This
> version starts with an empty library and does not bring over what an earlier
> version kept.

## Where your library lives

On the desktop, your library is a folder in drawbar's own data:

- macOS: `~/Library/Application Support/drawbar/library`
- Linux: `~/.local/share/drawbar/library`
- Windows: `%LOCALAPPDATA%\drawbar\library`

**File ▸ Show the library folder** opens it. Finder, your backups and Nord
Sample Editor see the same files and folders drawbar does. drawbar keeps your
tags, and where each sound came from, in a hidden `.drawbar` folder beside them,
so back up the whole library folder. That folder says nothing about your other
files, so it stays small however large the library is. Your unsaved edits are
kept on this computer, in drawbar's own data, never in the library folder.

In the browser, your library is kept in the browser's own storage for
drawbar.app, which no other site or app can see. It keeps your files, tags and
unsaved edits from one visit to the next, but the browser can delete it, as
[What is supported](../getting-started/support.md#the-library-in-the-browser)
explains, and clearing the site's data deletes it too. drawbar says so once each
visit, and hovering **This computer** shows what may delete it. **Help ▸ About
drawbar** shows how much space it takes; see [Help and About](help.md).

## Keeping a copy in the browser

**File ▸ Save a copy of the library** downloads the whole library as one ZIP
file, named for the day, such as `drawbar library 2026-10-30.zip`. It holds your
files, folders and tags. Unsaved edits stay in the browser. Change nothing in the
library until the download finishes, or the copy may not be whole.

To bring a copy back, choose **File ▸ Open a copy…** while the browser's library
holds none of your files: after the browser deleted it, or in another browser.
drawbar opens only a copy it saved, and only into a library with none of your
files in it, so it never mixes a copy with files already there. Close drawbar in
the browser's other tabs first. If opening a copy stops partway, the library opens
read-only until you open the copy again. A web
app added to the Dock or the Home Screen keeps storage of its own, apart from
Safari's, so save a copy in Safari and open it in the web app. In Chrome and
Edge you can also unzip the copy and open the folder with **File ▸ Open library
folder…**.

## Opening another folder

On the desktop, **File ▸ Open library folder…** opens any folder as your
library: a Sample Editor project, a sample pack, a folder on a shared drive.
**File ▸ Open recent library** switches back. drawbar opens the last library
again when it starts, and changes nothing in a folder until you change something
there. Unsaved edits stay with their library and come back when you open it
again.

In the browser, browsers that can open a folder on your computer offer **File ▸
Open library folder…** too ([What is supported](../getting-started/support.md)
lists them). The browser asks whether drawbar may change the folder. It forgets
that answer when drawbar.app closes, unless you allowed it on every visit, so the
next visit starts with the browser's own library, and **File ▸ Reconnect**
*folder* asks again. If you take the permission back while the folder is open,
drawbar closes it the same way.

The library folder can live in Dropbox, OneDrive or iCloud Drive, or on a
network share, and syncing just works: a copy that is out of date, changes that
arrive out of order, and a folder partly restored from a backup all come
together again. Each drawbar keeps its own record of what it changed in the
hidden folder, so none overwrites another's. That is also why several drawbars
can use one library at once: on two computers, in two windows, in two browser
tabs, or in the browser and the desktop app on one folder. A tab shows another
tab's changes as soon as they are made; otherwise drawbar shows what the others
changed when you come back to the window, and the activity log says so.

In the browser, and on a desktop disk that cannot refuse to move a file onto one
already there, drawbar checks that a name is free before a saved, copied or
moved file takes it. A file another program makes at that name in the instant
between is replaced, and what it held is lost.

## Adding and making sounds

Drop files on the window, or use **File ▸ Open…**. drawbar copies them into your
library and leaves the originals where they were. A file drawbar cannot read
still shows up, and the activity log says what went wrong.

**New** makes a fresh sound, note or folder. A fresh Stage program has every
control at zero; it is not a factory sound.

## Saving

Edits land at once, and the name shows a `*` until you save. **Save** (⌘S)
writes the file, and queues it for sending if the sound came from a slot on the
connected instrument. **Revert** goes back to the last save.

Unsaved edits are kept when you quit: those of sounds in your library, and those
of a sound you were viewing in a slot of the instrument. A slot's edits come
back when you view that slot of the same instrument again with the same library
open; if the slot changed since, drawbar asks which to keep. drawbar also keeps
edits every few seconds, so after a crash you may lose the last few seconds. In
the browser, closing the tab before drawbar has kept your latest edit makes the
browser ask whether to leave. Stay a moment, and the edit is kept. A read-only
library keeps no unsaved edits, unless a change drawbar could not finish made it
read-only.

Unsaved edits belong to this computer's copy of drawbar, not to one window. A
window or tab that opens the library shows the edits kept for it, including
those of another window still open. Where two windows kept edits of one sound,
the one kept last is shown, and the activity log says so.

## Undo

**Edit ▸ Undo** (⌘Z) takes back the last change you made to your files and
tags, and **Edit ▸ Redo** (⇧⌘Z) makes it again. Undo reaches back only to when
this window opened the library: a change from before, or from another window,
cannot be undone. The menu says which change it is, such as **Undo Rename
“c3.ne5p”**. A save, a new sound, a copy, a rename or move, a deletion, a new or
removed folder, and a change to tags can each be undone. One action is one
step: dropping, moving or deleting several sounds at once, unpacking a bundle,
writing over a sound, removing a folder with what is in it, or making a tag and
naming it, is one undo. An action on more files than the library can keep as one
step is refused whole and changes nothing; do it in smaller parts. That is tens of
thousands of files with short names, and about ten thousand with names hundreds
of characters long. If another drawbar has changed what an undo would take
back, that undo is refused, and Undo goes on to the change before it. An edit
you have not saved is taken back with **Revert** instead.

Deleting a sound, or saving over one, moves the old file into drawbar's trash in
the hidden folder, where Undo finds it. Undo reaches only the changes made since
the library opened, and the trash keeps only what they need, at most 1 GiB. Past
that, the oldest goes first, and the activity log says which change Undo no
longer reaches. Quitting empties the trash. What a crash left there goes when
the library next opens, once drawbar has finished any change the crash
interrupted.

**Export…** saves a copy somewhere else. Rename with F2. If a name is taken,
drawbar asks whether to **Overwrite** the file there or **Keep both**.

## Bundles

A bundle (`.ne5pbundle` or `.ne5tbundle`) is the file Nord Sound Manager uses to
carry programs, or a set list, together with the pianos and samples they play.
Drop one on the window, or open it with **File ▸ Open…**, and drawbar unpacks it
into a new folder named after the bundle. The bundle itself is not kept.

To make one, check the sounds to bundle and choose **Export as bundle…**. They
can be on this computer, on the instrument, or both. drawbar adds what they
need: the programs a set list plays, and the pianos and samples a program plays.
Sounds on the instrument are copied to this computer first. In the browser the
bundle arrives in your downloads.

A program's file names its piano and sample by a number only the instrument can
put a name to. A piano or sample already on this computer is added when the
instrument has named it, which happens when drawbar copies the program from its
slot. The activity log lists anything the bundle had to leave out.

## Changes made outside drawbar

You can rename, move, edit and delete your files in Finder or any other app.
drawbar notices when you come back to its window or tab, and a renamed or moved
sound keeps its tags and where it came from.

drawbar never writes over a change made outside it without asking. If a file
changed while you had unsaved edits to it, drawbar asks what to do:

- **Keep mine** keeps your edits. Your next save replaces the file.
- **Take theirs** drops your edits and uses the file as it is now.
- **Keep both** saves your edits as a new file beside it.

A sound that changed on disk is not sent to the instrument until you have looked
at it.

## Kept apart

drawbar keeps a history of the changes made to a library, such as tags added and
files renamed, so that every computer sharing the folder agrees on them.
Sometimes part of that history cannot be merged with the rest. **Kept apart**
then appears in the sidebar, and the activity log says so once. Click it to see
each group of changes: who made them, when, and what each one does.

This happens when:

- drawbar's own data was copied to a second computer, and both copies went on
  making changes. Each copy's changes are a branch of their own, and the
  activity log says that this drawbar goes on with a history of its own.
- A sync has brought later changes before the earlier ones they follow. These
  wait until the rest arrives and are merged then. Meanwhile they can be taken
  but not dismissed.
- A sync or a backup replaced part of the history with an older version, leaving
  changes only that version holds. These can be taken or dismissed.

Nothing kept apart changes your library until you choose to:

- **Take** makes one change. **Take all** makes every change in the group. Each
  is one step that **Edit ▸ Undo** takes back. Taking a change to a sound the
  branch added brings the sound too, at its file. Where your library already has
  a sound at that file, the change is made to it.
- Taking a change to where a sound's file is makes the sound's file the one at
  that path. The file must be there, as drawbar last found it, and belong to no
  other sound; otherwise the activity log says why it was not taken.
- **Dismiss…** stops showing a branch, on this computer and every other. It
  cannot be undone. A branch whose changes you have all taken says there is
  nothing left to review, and offers only this. A branch you dismissed can show
  again once the drawbar that made it goes on with it; dismiss it again if you
  still don't want it.
- **Show file** shows a sound a branch left without a file, which cannot be
  taken.

A change marked “only taking it settles this” replaced a value that no computer
shows any more, so it stays listed until you take it.

When you select a sound with changes kept apart, the inspector says so and
links to them.

## Demo sounds

**Start with a demo** on the welcome sheet (**Help ▸ Welcome**) downloads a tine
electric piano and a looped pad into a **Demo sounds** folder. Ask again to
bring back any you deleted.

## If something goes wrong

- **Read-only.** drawbar cannot write the library folder, a newer drawbar has
  written its hidden folder, or drawbar could not finish a change, as when
  another program put a file where a save was going. Hover the library's name
  for the reason and what to do, then choose the library in **File ▸ Open recent
  library** to open it again. Changes you make in a read-only library are held
  in memory only, and are lost when you quit; after a change drawbar could not
  finish, unsaved edits are still kept.
- **A change left unfinished.** If drawbar on another computer, or in a browser
  on this one, stopped in the middle of changing files in a shared library,
  drawbar asks once the change has stood for a few seconds: **Finish** completes
  the change, **Roll back** puts the files back as they were, **Leave as is**
  stops asking, and **Later** asks next time. What
  drawbar left unfinished on this computer, after a crash, a failed save or in
  another window, is finished without asking, by its next change or when the
  library next opens. If it can no longer be finished, because a file it was
  moving is gone, drawbar asks whether to roll it back or leave it as is, and
  Undo no longer reaches the changes before it.
- **An older drawbar.** A drawbar from before this version keeps its own tags,
  unsaved edits and rescued slot contents in the `.drawbar` folder. The first
  time this version opens such a library where it can write, it takes in the
  tags and where each file came from, once, and the activity log says for how
  many files. It leaves the unsaved edits and rescued slot contents where they
  are, and says so once. An older drawbar that opens this library sees its files
  without the tags this version keeps.
  Unsaved edits an earlier drawbar kept on this computer are not read either, and
  the activity log says so once.
- **A folder that stays.** Where a folder moves one file at a time, as in the
  browser, a folder that still holds a file drawbar does not list, such as one
  whose name starts with a dot, stays where it was, and the activity log names
  it.
- **Not answering.** If the library shows no sign of life for a minute, drawbar
  asks whether to close and reopen it. Edits made since drawbar last kept them
  are lost, and drawbar names them. If the library stops altogether, drawbar
  closes it and says so; **File ▸ Open recent library** opens it again.
- **Restored from a backup.** When the library folder no longer holds changes
  drawbar showed before, as after a restore from a backup, drawbar shows the
  folder as it is the next time it looks. While a sync is still bringing the
  folder's files, changes can disappear the same way and come back once they
  arrive. A change you make meanwhile is kept: at worst it shows beside the one
  the sync brings.
- **Missing.** The file was deleted outside drawbar, but it had tags or unsaved
  edits, or came from your keyboard. Save it to bring the file back, or delete
  it. After a restart, a missing file with tags shows as lost, with its tags,
  until you let it go.
- **A write to the instrument left a file.** If drawbar stopped, or could not
  put the old sound back, while it replaced a sound on your keyboard, the sound
  that was there may now be only on this computer. drawbar puts it in the
  library as a file of its own, named after the slot it came from, once the
  instrument is idle or when the library next opens. Undo takes it out again,
  and you can delete it like any other file. While the library cannot be
  written, drawbar keeps it aside and asks: **Show the file** shows where it is,
  **Discard** deletes it, and **Later** leaves it for the next library that can
  take it. A file that does not read is not a copy of the sound, and drawbar
  offers only to discard it.
- **Differs only by case.** Two names in one folder differ only in upper and
  lower case, such as `Pad.ne5p` and `pad.ne5p`. macOS and Windows hold only one
  of them, so rename one to keep the library whole on every computer.
- **Not read** or **not all listed.** drawbar could not read that file or
  folder, and the activity log names it. What is in such a folder is not shown
  until drawbar can read it.
- **Failed verification.** The sample or piano file is damaged. drawbar checks
  one when you open or send it, and will not send a damaged one.
- **No storage in the browser.** Some private windows give drawbar none. drawbar
  says so, and what you make lasts only until the tab closes. drawbar also cannot
  keep a copy of a sound it would replace, so it sends only to empty slots.

How the library works underneath is in
[Library data model](../reference/data-model.md).
