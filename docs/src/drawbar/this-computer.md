# Your files

**This computer** is your library: the sounds you open, make, or copy off an
instrument. Each one is a real file in a folder you can see.

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
drawbar.app, where nothing else can see it. Clearing the site's data deletes it,
so export what you want to keep.

## Opening another folder

**File ▸ Open library folder…** opens any folder as your library: a Sample
Editor project, a sample pack, a folder on a shared drive. **File ▸ Open recent
library** switches back. drawbar opens the last library again when it starts,
and changes nothing in a folder until you change something there. Unsaved edits
stay with their library and come back when you open it again.

Several drawbars can use one library at once: on two computers that sync the
folder, or in two windows. Each keeps its own record of what it changed in the
hidden folder, so neither overwrites the other's. When you come back to the
window, drawbar shows what the others changed, and the activity log says so.

This works on the desktop and in Chrome and Edge. Firefox and Safari cannot open
a folder. Brave can once you turn on `brave://flags/#file-system-access-api` and
relaunch it. In Brave, saving or renaming a large piano takes a little longer.

The first time you open a folder in the browser, drawbar reminds you not to open
it in the desktop app at the same time.

In Chrome and Edge, the browser asks whether drawbar may change the folder.
Unless you allow it on every visit, it forgets when you close drawbar.app.
Choose **File ▸ Reconnect** and the folder's name to get it back.

Chrome makes each file and folder drawbar writes there readable only by you, and
on macOS marks it as downloaded, so in a shared folder other people lose access
to what you save.

## Adding and making sounds

Drop files on the window, or use **File ▸ Open…**. drawbar copies them into your
library and leaves the originals where they were. A file drawbar cannot read
still shows up, and the activity log says what went wrong.

**New** makes a fresh sound, note or folder. A fresh Stage program has every
control at zero; it is not a factory sound.

## Saving

Edits land at once, and the name shows a `*` until you save. **Save** (⌘S)
writes the file, and queues it for sending if the sound came from a slot on the
connected instrument. **Revert** goes back to the last save. Unsaved edits are
kept when you quit. drawbar also keeps them every few seconds, so after a crash
you may lose the last few seconds.

## Undo

**Edit ▸ Undo** (⌘Z) takes back the last change you made to your files and
tags in this session, and **Edit ▸ Redo** (⇧⌘Z) makes it again. The menu says
which change it is, such as **Undo Rename “c3.ne5p”**. A save, a new sound, a
copy, a rename or move, a deletion, and a change to tags can each be undone, one
at a time. An edit you have not saved is taken back with **Revert** instead.

Deleting a sound moves its file into drawbar's trash in the hidden folder, where
Undo finds it. drawbar empties the trash of what was deleted more than 30 days
ago, and of the oldest past 1 GB, when the library opens.

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

## Demo sounds

**Start with a demo** on the welcome sheet (**Help ▸ Welcome**) downloads a tine
electric piano and a looped pad into a **Demo sounds** folder. Ask again to
bring back any you deleted.

## If something goes wrong

- **Read-only.** drawbar cannot write the library folder, or a newer drawbar
  has written its hidden folder. Hover the library's name for the reason.
  Changes you make in a read-only library are held in memory only, and are lost
  when you quit.
- **Restored from a backup.** When the library folder no longer holds changes
  drawbar showed before, as after a restore from a backup, drawbar shows the
  folder as it is and says so.
- **Missing.** The file was deleted outside drawbar, but it had tags or unsaved
  edits, or came from your keyboard. Save it to bring the file back, or delete it.
  After a restart, a missing file with tags shows as lost, with its tags, until
  you let it go.
- **A write to the instrument left a file.** If drawbar stopped, or could not
  put the old sound back, while it replaced a sound on your keyboard, the sound
  that was there may now be only on this computer. drawbar offers it when the library opens: **Keep in
  library** adds it to your sounds so you can send it back, **Show the file**
  shows where it is, and **Discard** deletes it. A file that does not read is
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
