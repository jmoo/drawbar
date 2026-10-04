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
Sample Editor see the same files and folders drawbar does. drawbar keeps tags
and unsaved edits in a hidden folder beside them, so back up the whole library
folder. That folder holds nothing about your other files, so it stays small
however large the library is.

In the browser, your library is kept in the browser's own storage for
drawbar.app, where nothing else can see it. Clearing the site's data deletes it,
so export what you want to keep.

## Opening another folder

**File ▸ Open library folder…** opens any folder as your library: a Sample
Editor project, a sample pack, a folder on a shared drive. **File ▸ Open recent
library** switches back. drawbar opens the last library again when it starts,
and changes nothing in a folder until you change something there. Unsaved edits
stay with their library and come back when you open it again.

This works on the desktop and in Chrome and Edge. Firefox and Safari cannot open
a folder. Brave can once you turn on `brave://flags/#file-system-access-api` and
relaunch it.

The first time you open a folder in the browser, drawbar reminds you not to open
it in the desktop app at the same time.

In Chrome and Edge, the browser asks whether drawbar may change the folder.
Unless you allow it on every visit, it forgets when you close drawbar.app.
Choose **File ▸ Reconnect** and the folder's name to get it back.

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
It is the only undo. Unsaved edits are kept when you quit. drawbar also keeps
them every few seconds, so after a crash you may lose the last few seconds. In
the browser, closing the tab before drawbar has kept your latest edit makes the
browser ask whether to leave. Stay a moment, and the edit is kept.

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
drawbar notices when you come back to its window, and a renamed sound keeps its
tags. drawbar reads a tagged file once in the background; a rename before that
shows as a new file.

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

- **Read-only.** Another copy of drawbar has this library open, in another window
  or browser tab. Close it. Hover the library's name for the reason. A browser
  tab cannot tell that the desktop app has a folder open, so open a folder in
  only one of them at a time. drawbar also opens a library read-only when its
  hidden folder has lost the file that lists your unsaved edits, so they are not
  deleted. On the desktop, put that file back from a backup to get them back.
  **Open without them** deletes those edits and opens the library as usual.
- **Missing.** The file was deleted outside drawbar, but it had tags or unsaved
  edits, or came from your keyboard. Save it to bring the file back, or delete it.
- **A write to the instrument left a file.** If drawbar stopped, or could not
  put the old sound back, while it replaced a sound on your keyboard, the sound
  that was there may now be only on this computer. drawbar offers it when the library opens: **Keep in
  library** adds it to your sounds so you can send it back, **Show the file**
  shows where it is, and **Discard** deletes it.
- **Not read** or **not all listed.** drawbar could not read that file or
  folder.
- **Failed verification.** The sample or piano file is damaged. drawbar will not
  send it.
- **No storage in the browser.** Some private windows give drawbar none. drawbar
  says so, and what you make lasts only until the tab closes. drawbar also cannot
  keep a copy of a sound it would replace, so it sends only to empty slots.

How the library works underneath is in
[Library data model](../reference/data-model.md).
