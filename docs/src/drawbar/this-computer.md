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
unsaved edits from one visit to the next, but clearing the site's data in the
browser deletes them, so export what you want to keep elsewhere. **Help ▸ About
drawbar** shows how much space it takes, and whether the browser has promised to
keep it; see [Help and About](help.md).

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

Several drawbars can use one library at once: on two computers that sync the
folder, in two windows, in two browser tabs, or in the browser and the desktop
app on one folder. Each keeps its own record of what it changed in the hidden
folder, so neither overwrites the other's. A tab shows another tab's changes as
soon as they are made; otherwise drawbar shows what the others changed when you
come back to the window, and the activity log says so.

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
library keeps no unsaved edits.

## Undo

**Edit ▸ Undo** (⌘Z) takes back the last change you made to your files and
tags, and **Edit ▸ Redo** (⇧⌘Z) makes it again. Undo reaches back only to when
this window opened the library: a change from before, or from another window,
cannot be undone. The menu says which change it is, such as **Undo Rename
“c3.ne5p”**. A save, a new sound, a copy, a rename or move, a deletion, a new or
removed folder, and a change to tags can each be undone. One action is one
step: dropping, moving or deleting several sounds at once, unpacking a bundle,
writing over a sound, removing a folder with what is in it, or making a tag and
naming it, is one undo. An edit you have not saved is taken back with **Revert**
instead.

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
  wait until the rest arrives and are merged then, so there is nothing to do
  with them.
- A sync replaced part of the history with an older version, leaving changes
  only that version holds. These can be taken but not dismissed.

Nothing kept apart changes your library until you choose to:

- **Take** makes one change. **Take all** makes every change in the group. Each
  is one step that **Edit ▸ Undo** takes back.
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

A card may say its changes could be that drawbar's own older changes, which it
has replaced since. Taking one of them may bring back what it replaced.

A change marked “only taking it settles this” replaced a value that no computer
shows any more, so it stays listed until you take it.

When you select a sound with changes kept apart, the inspector says so and
links to them.

## Demo sounds

**Start with a demo** on the welcome sheet (**Help ▸ Welcome**) downloads a tine
electric piano and a looped pad into a **Demo sounds** folder. Ask again to
bring back any you deleted.

## If something goes wrong

- **Read-only.** drawbar cannot write the library folder, or a newer drawbar
  has written its hidden folder. Hover the library's name for the reason.
  Changes you make in a read-only library are held in memory only, and are lost
  when you quit.
- **A change left unfinished.** If drawbar on another computer, or in a browser
  on this one, stopped in the middle of changing files in a shared library,
  drawbar asks once the change has stood for a few seconds: **Finish** completes
  the change, **Roll back** puts the files back as they were, **Leave as is**
  stops asking, and **Later** asks next time. What
  drawbar left unfinished on this computer, after a crash or in another window,
  is finished without asking.
- **An older drawbar.** A drawbar from before this version keeps its own tags,
  unsaved edits and rescued slot contents in the `.drawbar` folder. This version
  says so once and leaves them where they are, and an older drawbar that opens
  this library sees its files without the tags this version keeps.
- **A folder that stays.** Where a folder moves one file at a time, as in the
  browser, a folder that still holds a file drawbar does not list, such as one
  whose name starts with a dot, stays where it was, and the activity log names
  it.
- **Not answering.** If the library shows no sign of life for a minute, drawbar
  asks whether to close and reopen it. Edits made since drawbar last kept them
  are lost, and drawbar names them. If the library stops altogether, drawbar
  closes it and says so; **File ▸ Open recent library** opens it again.
- **Restored from a backup.** When the library folder no longer holds changes
  drawbar showed before, as after a restore from a backup, drawbar keeps showing
  them for a day, in case a sync has not brought them yet. After that, and once
  drawbar has looked at every file again, it shows the folder as it is.
- **Missing.** The file was deleted outside drawbar, but it had tags or unsaved
  edits, or came from your keyboard. Save it to bring the file back, or delete
  it. After a restart, a missing file with tags shows as lost, with its tags,
  until you let it go.
- **A write to the instrument left a file.** If drawbar stopped, or could not
  put the old sound back, while it replaced a sound on your keyboard, the sound
  that was there may now be only on this computer. drawbar offers it when the
  library opens: **Keep in library** adds it to your sounds so you can send it
  back, **Show the file** shows where it is, and **Discard** deletes it. A file that does not read is
  not a copy of the sound, and drawbar offers only to discard it.
- **Differs only by case.** Two names in one folder differ only in upper and
  lower case, such as `Pad.ne5p` and `pad.ne5p`. macOS and Windows hold only one
  of them, so rename one to keep the library whole on every computer.
- **Not read** or **not all listed.** drawbar could not read that file or
  folder, and the activity log names it. What is in such a folder is not shown
  until drawbar can read it.
- **Failed verification.** The sample or piano file is damaged. drawbar will not
  send it.
- **No storage in the browser.** Some private windows give drawbar none. drawbar
  says so, and what you make lasts only until the tab closes. drawbar also cannot
  keep a copy of a sound it would replace, so it sends only to empty slots.

How the library works underneath is in
[Library data model](../reference/data-model.md).
