# Library data model

drawbar keeps This computer as a folder of real files, so that a library folder
Dropbox, OneDrive, iCloud or a network share syncs just works. Sync goes wrong in
ordinary ways, even for someone who runs one drawbar at a time. Another computer
holds stale state, files arrive out of order, a backup restores part of the
folder, or a conflicted copy appears.

drawbar holds to four things through all of these:

- normal syncing needs no intervention;
- once a sync settles, every drawbar shows the same library;
- an interrupted or conflicting change leaves every sound file recoverable;
- when recovery is needed, drawbar says what happened without asking the user
  to understand its log.

The rest of this page describes how: what is stored where, how a change is
made, and how drawbar stays in step with sync and with other programs. What a
user sees of it is on [Your files](../drawbar/this-computer.md).

The code is in `crates/drawbar/src`: `store/` holds the library on disk,
`workspace.rs` the assets in memory, and `ondisk.rs` the files read by range.
Every change to the files, and everything a file cannot say about itself, goes
through [toshokan](https://github.com/jmoo/drawbar/tree/master/crates/toshokan).
Its [`SPEC.md`](https://github.com/jmoo/drawbar/blob/master/crates/toshokan/SPEC.md)
is the precise version of most of this page.

## What lives where

| What | Desktop | Browser |
|---|---|---|
| Library files | The library folder | The private file system's root, or a picked folder |
| Tags, origins, each drawbar's log, staging and trash | `.drawbar/library/` in the library | `.drawbar/library/` in the library |
| Unsaved edits, identities, rescued slot contents | `libraries/<hash>/` beside `app.ron` | `.drawbar/local/private/` or `.drawbar/local/picked-<id>/` of the private file system |
| Theme, panels and other preferences | eframe's store: `app.ron` in drawbar's data folder | eframe's store, in local storage |
| MIDI on or off, recent libraries | eframe's store | |
| Recent picked folders | | IndexedDB, as folder handles |

On the desktop the default library is `library` in drawbar's data folder; the
paths are on [Your files](../drawbar/this-computer.md#where-your-library-lives).
In the browser it is the root of the origin private file system. A preference is
never kept in a library, and a library is never kept in preferences.

```text
<library>/
  .drawbar/
    library/                toshokan's root
      writers.2/<writer>/   one drawbar's log, staging, pending changes and trash
  Grand.npno
  Cello/c3.wav
```

## What the library keeps

A library is a folder tree. A folder in the sidebar is a directory, an asset is
a file, and a move is a rename. Any folder can be opened as a library.

What a file cannot say lives in `.drawbar/library/`: an asset's tags, where it
came from (New, a slot, or a rescue), and the library's list of tags. toshokan
keeps these as facts about entities. An entity exists only for a file with
something to say, so a library of untouched files keeps nothing about them.

toshokan binds each entity to its file by path. Where the file moved, it finds
the file by its identity: a BLAKE3 hash of its length and bytes. A Nord file's
identity reads only its header and its checksum, so a large piano library costs
a small read. [Binding](https://github.com/jmoo/drawbar/blob/master/crates/toshokan/SPEC.md#binding)
has the rules.

The hidden folder is made at the first change, so a folder opened and looked at
stays as it was. A scan leaves out the hidden folder and every other name that
starts with a dot.

Every name drawbar makes follows the strictest rules of macOS, Windows and
Linux, so a library can move between them. Two names in one folder collide when
they differ only in case or Unicode normalization (`store/names.rs`).

Losing `.drawbar/` loses the tags, the origins and the trash that undo needs.
Every other file stays as it was.

## What this install keeps

What only this install needs is kept in drawbar's own data, never in the
library. It holds toshokan's local root, which names the writers this install
owns and remembers the identities its scans read. It also holds the unsaved
edits, and `rescued/` for a slot's former occupant. Losing it loses the unsaved
edits and any rescued occupant not yet taken into the library. The next change
then writes as a new writer.

drawbar keeps each unsaved edit every few seconds and at exit, so a crash loses
only the edits made since the last keep. An asset's edit is kept with the
identity of the file it was made over. A view of a slot has no file, so its
edit is kept by its slot, with the hash of the bytes it was opened over. An edit
of a piano or sample instrument resting in its file is kept as the edit itself,
and made again over the file at the next open. Each keep is a new file that
never changes, and the latest keep of an edit wins, so the windows and tabs of
one install share edits without locks (`store/unsaved.rs`). A library that
cannot be written keeps no unsaved edits, and says so when it opens.

Before a write to the instrument replaces a slot's occupant, drawbar reads the
occupant into `rescued/` and puts it back if the write fails
([Writing over a slot](usb-protocol.md#writing-over-a-slot)). A file still there
once the instrument is idle, or at the next open, may be the slot's only copy.
drawbar copies it into the library under the slot's name, as one change undo
can take back, and then deletes it. While the library cannot be written, the
file stays where it is, and drawbar asks about it.

A rescued occupant survives a power cut only where drawbar can sync its folder
to disk: on macOS and Linux, not on Windows or in the browser. In the browser,
the worker that writes the file loads from the bundle the page loaded, so after
drawbar.app is deployed again, a send that would replace an occupant can be
refused until the page is reloaded.

## How a change works

Every change the user makes to the files or the facts is an intent. An intent is
one undo step, and one gesture is one intent, such as dropping ten files or
making a tag and putting it on assets. Before anything is written, toshokan
checks each file the intent touches against what drawbar last knew of it: its
identity where drawbar has read or written the file, and its length and time
otherwise.

toshokan then stages the new bytes in the writer's `tmp/`, records the change as
pending, moves the bytes it displaces to the writer's `trash/`, and logs what it
did. A crash leaves each path holding its old bytes or its new ones, and the
next open finishes the change or rolls it back.
[File effects and pending records](https://github.com/jmoo/drawbar/blob/master/crates/toshokan/SPEC.md#file-effects-and-pending-records)
has the steps. drawbar never reads a file whole to write it: a copy, or an edit
of a resting file, streams a chunk at a time.

Undo reaches only this session's intents. It is refused where another drawbar
changed the same thing since
([Undo and redo](https://github.com/jmoo/drawbar/blob/master/crates/toshokan/SPEC.md#undo-and-redo)),
and drawbar then lets go of that step, so the next undo reaches the one before.
An intent too large for one log entry is refused before it writes anything.

The trash holds what a delete or a save displaced, for this session's undo and
nothing else. After each change drawbar keeps this session's items, newest
first, up to 1 GiB, and empties the rest. The Edit menu stops at the first
change whose item was emptied. Closing the library empties the trash, and so
does opening one that can be written, once any unfinished change is settled
([Trash](https://github.com/jmoo/drawbar/blob/master/crates/toshokan/SPEC.md#trash)).

The app never waits on the disk. `Store` turns the user's changes into intents
and sends them to a backend: a thread on the desktop, a worker in the browser.
Answers come back as events, and the store projects each new view of the
library onto the workspace. A fact shows once its intent has committed.

## Sync and several drawbars

Every running drawbar writes as a writer of its own. It appends to its own log
under `writers.2/` and reads the others' logs. Only a writer writes in its own
directory, so a sync client never has two versions of one of those files to
reconcile, and a conflicted copy of a log is read like any other file. toshokan
merges every log to the same state, whatever order a sync client delivers them
in, and keeps concurrent changes to one field as a conflict
([Writers](https://github.com/jmoo/drawbar/blob/master/crates/toshokan/SPEC.md#writers)).

Several drawbars on one library at once follow from the same design: two
windows, two tabs, or two computers that sync the folder. It is not meant for
live collaboration. The windows and tabs of one install share its local root. A
tab and the desktop app on one folder are two installs, as two computers are.

A drawbar sees the others' changes when it reads their logs again: when its
window comes back into focus, before a send to the instrument, and after a
failure that may mean the folder changed. A tab tells the library's other tabs
of each change it commits, and they read it at once.

A change another drawbar left unfinished is settled. One this install left,
after a crash or in a window since closed, is finished without asking. One
another install left is asked about: finish it, roll it back, or leave it.
drawbar asks only once the change has stood unmoved for five seconds, since a
drawbar in a browser tab may still be making it.

A library opens read-only when its folder cannot be written, or when this
writer's log holds entries a newer build wrote. Another drawbar on the folder
never makes it read-only. Changes in a read-only library are held in memory
only.

A reader keeps nothing between opens: what it shows comes from the files in the
folder. After a restore from a backup, facts no file holds now are gone at the
next read. While a sync client is still delivering files, facts can disappear
and come back, and a change made meanwhile is at worst a visible conflict
([What users see](https://github.com/jmoo/drawbar/blob/master/crates/toshokan/RETENTION.md#what-users-see)).

## Changes made outside drawbar

A rescan lists every file by name, length and time. It reads a file's identity
only where that decides which file is an entity's. An entity's file renamed or
moved outside drawbar is followed by its identity, and the asset keeps its tags
and its origin. A file with no entity is known by its path alone, so its rename
reads as one file gone and another arrived, with nothing lost.

A file that is gone, and that no new file matches, leaves the list if it held
nothing precious. One with tags, an unsaved edit, a slot it came off or a place
in the send queue stays, marked missing, and saving it writes the file anew.
After a restart, an entity with tags whose file is gone shows as a lost row
until the user forgets it.

A file changed on disk under an unsaved edit becomes the new baseline, and the
edit stays over it. The user then chooses which to keep
([Changes made outside drawbar](../drawbar/this-computer.md#changes-made-outside-drawbar)).
An edit held over a resting piano or sample instrument is made again over the
new file. Where it no longer applies, it stays unsaved and refuses to be saved
until it is edited again or reverted. A send waits for its rescan, and goes
ahead only if nothing it would send changed on disk.

## Kept apart

**Kept apart** lists what toshokan cannot place in a writer's single history,
shown apart from the merged state. Most of it is branches. A writer's history
forks when two installs go on from one copy of drawbar's own data, and each
branch's changes after the fork wait for the user. The rest is entries whose
predecessor has not synced yet, merged once it arrives, and lines of an older
copy of a log segment, as a sync client or a backup leaves. Of those lines, only
what the writer's later history never changed again is shown.

Taking changes is one intent, which undo takes back. Dismissing a branch, or the
lines of an older segment, hides it from every reader and cannot be undone. A writer that finds its
own history forked goes on as a new writer.
[Kept apart](../drawbar/this-computer.md#kept-apart) says what the user sees,
and [Runs](https://github.com/jmoo/drawbar/blob/master/crates/toshokan/RETENTION.md#5-runs)
how toshokan finds them.

## In the browser

The library is kept by a dedicated worker, started from drawbar's own bundle.
It runs the same code as the desktop's backend thread, over toshokan's browser
executor: sync access handles in the private file system, and writable streams
in a picked folder. Nothing needs shared memory, so the page needs no
cross-origin isolation. The page reads files itself, from the folder's handles,
so a file's bytes cross to the worker only to be written.

Other differences:

- A picked folder may be written by other programs. It is also rescanned when
  its tab is shown or focused, every minute while it is in view, and where
  Chromium's `FileSystemObserver` reports a change.
- A browser's rename can replace a file. toshokan checks that the destination is
  free and then moves, so a file another program makes there in between is
  replaced.
- Where a browser cannot rename files in a picked folder, every move copies the
  file and removes the old one
  ([What is supported](../getting-started/support.md#the-library-in-the-browser)).
- A folder handle can be kept only in IndexedDB, and it comes back without its
  permission. The browser asks again on a click of **File ▸ Reconnect**.
- The first write asks the browser to keep the storage
  (`navigator.storage.persist()`). Where the browser gives the page no storage,
  the library lasts until the tab closes.

[Browsers](https://github.com/jmoo/drawbar/blob/master/crates/toshokan/SPEC.md#browsers)
has what each browser can do.

## How reads stay cheap

Opening reads the logs and shows the assets they name, with their tags, at once.
A scan of every file follows. It costs one listing request per folder, and a
file's kind comes from its extension, so the scan reads no file's contents
except for an identity it needs.

A listed asset holds only its name, length and time until something needs its
contents. drawbar reads it when an act needs it, a tab shows it, the user
selects it, or its row is drawn with nothing known of it, most needed first.
After the listing, files with tags or a slot they came off are also read in the
background, so they match their slots on the instrument before anything shows
them.

A piano library or sample instrument is never held whole. It rests in its file,
indexed and read by range ([Large libraries](file-formats.md#large-libraries)).
Its checksum is checked in one streaming pass once something needs the verdict,
as opening or sending it does. A send reads it a transfer chunk at a time.

The assets hold at most 1 GiB of a library's files whole, and a resting file
counts for nothing. When a read would not fit, drawbar lets go of clean assets,
least recently needed first. An asset let go of keeps a summary of its read, so
its row still draws as it did and still matches its slot. A view, an unsaved
asset, and one open, selected or waiting to be sent are never let go of. A read
that cannot fit even then is tried again once room can be made.
