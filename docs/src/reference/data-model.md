# Library data model

drawbar keeps This computer as a folder of real files, so the folder can be
synced. This page describes what is stored where, how a change is made, and how
drawbar stays in step with sync and with other programs. What a user sees of it
is on [Your files](../drawbar/this-computer.md).

The code is in `crates/drawbar/src`: `store/` holds the library on disk,
`workspace.rs` the assets in memory, and `ondisk.rs` the files read by range.
Every change to the files, and everything drawbar keeps about them, goes
through [toshokan](https://github.com/jmoo/drawbar/tree/master/crates/toshokan),
whose crate documentation says what it guarantees when sync goes wrong. Its
[protocol](https://github.com/jmoo/drawbar/blob/master/crates/toshokan/spec/protocol.md)
has the rules this page relies on, and its
[format](https://github.com/jmoo/drawbar/blob/master/crates/toshokan/spec/format.md)
the bytes.

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
a small read. Which extensions read that way is fixed with the format, since a
change would make every tagged file of that kind look changed outside drawbar: a
Nord format added since is read whole. [Binding](https://github.com/jmoo/drawbar/blob/master/crates/toshokan/spec/protocol.md#binding)
has the rules.

The `.drawbar` folder is made at the first change, so a folder opened and looked
at stays as it was. A scan leaves out the `.drawbar` folder and every other name that
starts with a dot.

Every name drawbar makes follows the strictest rules of macOS, Windows and
Linux, so a library can move between them. Two names in one folder collide when
they differ only in case or Unicode normalization (`store/names.rs`).

Losing `.drawbar/` loses the tags, the origins and the trash that undo needs.
Every other file stays as it was.

## What this install keeps

What only this install needs is kept in drawbar's own data, never in the
library, in a folder for each library. On the desktop that folder is named by a
hash of the library's path, so a library moved or renamed gets a new one. In the
browser it is named by the id drawbar gives a picked folder, which a folder
picked again keeps only while it is on the recent list. It holds toshokan's local
root, which names the writers this install owns and remembers the identities its
scans read. It also holds the unsaved edits, and `rescued/` for a slot's former
occupant. Losing it loses the unsaved edits and any rescued occupant not yet taken
into the library. The next change then writes as a new writer.

drawbar keeps each unsaved edit every few seconds and at exit, so a crash loses
only the edits made since the last keep. An asset's edit is kept with the
identity of the file it was made over. A view of a slot has no file, so its
edit is kept by its slot, with the hash of the bytes it was opened over. An edit
of a piano or sample instrument resting in its file is kept as the edit itself,
and made again over the file at the next open. Each keep is a new file that
never changes, and the latest keep of an edit wins, so the windows and tabs of
one install share edits without locks (`store/unsaved.rs`). A library that
cannot be written keeps its unsaved edits the same way.

Before a write to the instrument replaces a slot's occupant, drawbar reads the
occupant into `rescued/` and puts it back if the write fails
([Writing over a slot](usb-protocol.md#writing-over-a-slot)). A file still there
once the instrument is idle, or at the next open, may be the slot's only copy.
drawbar copies it into the library under the slot's name, as a change undo does
not take back, since the copy is then the only one, and deletes it. While the
library cannot be written, the file stays where it is, and drawbar asks about it.

A rescued occupant survives a power cut only where drawbar can sync its folder
to disk: on macOS, Linux and Windows, not in the browser. In the browser,
the worker that writes the file loads from the bundle the page loaded, so after
drawbar.app is deployed again, a send that would replace an occupant can be
refused until the page is reloaded.

## The list an earlier version kept

drawbar 0.10 and earlier kept This computer in eframe's store, under
`drawbar.this_computer`, `drawbar.folders` and `drawbar.tags`: each asset's name,
origin and bytes, base64-encoded, and the folders and tags by asset id
(`store/older.rs`). At the first open of a library that may be written, once its
files are listed and no change of drawbar's own waits to be finished, drawbar
takes that list in as one intent, which no undo takes back: a directory for each
folder, a new file for each asset holding its saved bytes, its tags and its slot
origin, and the tag names. A file name is free in its folder and takes the
extension its bytes call for; bytes that do not decode are written as they are.
An unsaved edit is held over its new file and kept as any unsaved edit is.

The same intent adds the BLAKE3 hash of the three keys' text to the library's
`taken in` set, so a list is taken in once. The keys stay as they were, so
drawbar 0.10 run again finds its list, and a list it changes since has another
hash and is taken in again. `drawbar.taking_in` and `drawbar.taken_in` record
the take-in's progress in eframe's store, so that a launch that stops partway
puts the unsaved edits back at most once (`Store::take_in`). A take-in that
fails is tried again at the next launch. A key whose version line this build
does not know leaves the whole list unread.

## How a change works

Every change the user makes to the files or the facts is an intent. An intent is
one undo step, and one gesture is one intent, such as dropping ten files or
making a tag and putting it on assets. Before anything is written, toshokan
checks each file the intent touches against what drawbar last knew of it: its
identity where drawbar has read or written the file, and its length and time
otherwise.

toshokan then stages the new bytes, records the change as pending, moves the
bytes it displaces to the writer's trash, and logs what it did, so a crash or a
failed write leaves each path holding its old bytes or its new ones
([File effects and pending records](https://github.com/jmoo/drawbar/blob/master/crates/toshokan/spec/protocol.md#file-effects-and-pending-records)).
drawbar never reads a file whole to write it: a copy, or an edit
of a resting file, streams a chunk at a time.

Undo reaches only this session's intents. It is refused where another drawbar
changed the same thing since
([Undo and redo](https://github.com/jmoo/drawbar/blob/master/crates/toshokan/spec/protocol.md#undo-and-redo)),
or where its files are not as the intent left them. A step another drawbar changed
since, or whose trash item went, can never be taken back, and drawbar lets go of
it, so the next undo reaches the one before. A step refused for what the folder
holds now stays, and goes through once that is out of the way, or back as the step
left it.
An intent too large for one log entry is refused before it writes anything.

The trash holds what a delete or a save displaced, for this session's undo and
nothing else. After each change drawbar keeps this session's items, newest
first, up to 1 GiB, and empties the rest. The Edit menu stops at the first
change whose item was emptied. Closing the library empties the trash, and so
does opening one that can be written, once any unfinished change is settled
([Trash](https://github.com/jmoo/drawbar/blob/master/crates/toshokan/spec/protocol.md#trash)).

The app never waits on the disk. `Store` turns the user's changes into intents
and sends them to a backend: a thread on the desktop, a worker in the browser.
Answers come back as events, and the store projects each new view of the
library onto the workspace. A fact shows once its intent has committed.

## Sync and several drawbars

Every running drawbar writes as a writer of its own: it appends to its own log
under `writers.2/`, and toshokan merges every writer's log to the same state,
whatever order a sync client delivers them in
([Writers](https://github.com/jmoo/drawbar/blob/master/crates/toshokan/spec/protocol.md#writers)).

Several drawbars can use one library at once without error: two windows, two
tabs, or two computers that sync the folder. The windows and tabs of one install
share its local root. A tab and the desktop app on one folder are two installs,
as two computers are.

A drawbar sees the others' changes when it reads their logs again: when its
window comes back into focus, before a send to the instrument, and after a
failure that may mean the folder changed. A tab tells the library's other tabs
of each change it commits, and they read it at once.

Two drawbars can each make an entity of one file before either sees the other's,
as when both tag it while a sync is paused.
[Binding](https://github.com/jmoo/drawbar/blob/master/crates/toshokan/spec/protocol.md#binding)
says which keeps the file. A drawbar that can write commits one change joining
each such pair: the entity that keeps the file gains the tags and the origin it
lacks, and the other is deleted. That change is no step of Undo. A read-only
drawbar writes nothing and shows the tags of both on the file.

A change another drawbar left unfinished is settled
([File effects and pending records](https://github.com/jmoo/drawbar/blob/master/crates/toshokan/spec/protocol.md#file-effects-and-pending-records)).
One this install left, after a crash or in a window since closed, is finished
without asking. One another install left is asked about: finish it, roll it
back, or leave it. One of this install's own that can no longer be finished,
since a file it moves is gone or was changed since the change checked it, is
asked about too: roll it back or leave it. Its writer stops writing, so a new
one goes on, and Undo starts again. Settling a change is no step of Undo.
drawbar asks only once the change has stood unmoved for five seconds, since a
drawbar in a browser tab may still be making it.

A library opens read-only when its folder cannot be written, when this writer's
log holds an entry of a kind or with an operation a newer build added, or when
the folder holds a newer layout of the `.drawbar` folder beside this one. What
may change between versions without that, and what may not, is in
[Compatibility](https://github.com/jmoo/drawbar/blob/master/crates/toshokan/spec/format.md#compatibility).
It turns read-only until it is opened again when a change it began cannot be
finished, as when another program put a file where the change puts one; opening
it again tries once more. It also turns read-only until it is opened again when
its folder keeps failing to make a write durable
([Writers](https://github.com/jmoo/drawbar/blob/master/crates/toshokan/spec/protocol.md#writers)).
Another drawbar on the folder never makes it read-only. Changes in a read-only library
are held in memory only, except edits, unsaved or saved there, which this
install's own state keeps as unsaved edits.

A reader keeps nothing between opens: what it shows comes from the files in the
folder, so a restore from a backup, or a sync still delivering files, changes
what it shows at the next read
([If something goes wrong](../drawbar/this-computer.md#if-something-goes-wrong)).
A file of another writer that cannot be read, as one a sync client keeps only
online, counts as not delivered yet, and no unfinished change is offered to
settle until it is read. One of the reader's own writer stops the library
opening.

## Changes made outside drawbar

A rescan lists every file by name, length and time, and reads a file's identity
only where that decides which file is an entity's, so an asset renamed or moved
outside drawbar keeps its tags and its origin
([Binding](https://github.com/jmoo/drawbar/blob/master/crates/toshokan/spec/protocol.md#binding)).
Each file the view shows is a row with a number of its own, which continues by
drawbar's own changes, by its entity's binding, or at the same path
([Rows](https://github.com/jmoo/drawbar/blob/master/crates/toshokan/spec/protocol.md#rows)),
and each asset stands for one row. A file with no entity is known by its path
alone, so its rename outside drawbar or by another drawbar ends its row, and the
file at the new path is a new asset. Logs only say who changed a row.

A file that keeps its length and time is read again only where another drawbar's
log says it wrote or moved a file there, once, at the next scan. A rewrite whose
file arrives after the scan of the refresh that read its entry shows at the file's
next change, or when a save of it is tried. A save of a file drawbar has read
compares its contents, so that save is refused: drawbar says the file changed on
disk, keeps the edit and asks whose to keep. A save of a file drawbar has not read
compares only its length and time. Where another file lands over a save before
drawbar looks, drawbar shows that file, unless it has the save's length and, where
the folder keeps a moved file's time, its time. A save drawbar finishes after
restarting, which it left part done, takes the file it finds there for its own.

Where several files could each be an entity's moved file, or one file several
entities', none is bound, and an entity with tags shows as a lost row. Where two
drawbars moved or rewrote one entity's file before either saw the other's change
and both files are there, the entity's file is the one the later change names
that still holds what it says; the other is a file with no entity.

An asset whose row ends, or whose entity's file is nowhere, leaves the list unless
it holds work no file does: an unsaved edit, a save not yet written, a slot it came
off, a place in the send queue, or a kept edit waiting for its file. Such an asset
stays, marked missing, in the nearest folder still there, and saving it writes it
there anew, beside any file that took its name. One whose row ended is an
unsaved copy: a file that arrives at its path is read, and is its file again only
where it holds what the copy's edit was made over; otherwise saving the copy writes
it anew, beside any file that took its name, even where it holds what another file
does. An entity with tags whose file is nowhere shows as a lost row
until its file is back or the user forgets it. A path a row whose file is nowhere
holds is taken: a new or renamed file is given another name, so a file still in
transit never becomes the new file's entity. Where two rows meet on one file and
both assets hold an unsaved edit, the older edit is kept apart as an unsaved copy
with no file.

drawbar names nothing a file with no entity went through. Of an entity's file,
the activity log says when another drawbar deleted it, and when another drawbar
moved it and the file has not arrived. Of an asset holding an unsaved edit whose
file is gone, it says that drawbar still holds the edit. Tagged files gone with
no word of why are counted in one line.

An unsaved edit is kept by the entity of its file, else by its path. At open the
backend gives it the row of that entity, following a join, or of the file at that
path where the file holds what the edit was made over, and it is put back over that
row's asset. Any other, and the older of
two that meet on one asset, comes back as an unsaved copy at its last path, or
beside it where another file holds that name.

Each answer that writes a file names the file's row. Where the app loses an answer,
the next view names the row of each file that answer wrote, and the asset takes that
row and reads its file again quietly; drawbar never takes a file by its path alone.

A file changed on disk under an unsaved edit becomes the new baseline, and the
edit stays over it. The user then chooses which to keep
([Changes made outside drawbar](../drawbar/this-computer.md#changes-made-outside-drawbar)).
An edit held over a resting piano or sample instrument is made again over the
new file. Where it no longer applies, it stays unsaved and refuses to be saved
until it is edited again or reverted. A send waits for its rescan, and goes
ahead only if nothing it would send changed on disk.

## Kept apart

**Kept apart** lists what toshokan cannot place in a writer's single history.
Most of it is runs: the branches of a writer's history after two installs went
on from one copy of drawbar's own data, each as a new writer. The rest is gaps,
entries whose predecessor has not synced yet, merged once it arrives, and
unplaced lines of an older copy of a log segment, as a sync client or a backup
leaves.
[Kept apart](../drawbar/this-computer.md#kept-apart) says what the user sees and
does, and [Runs](https://github.com/jmoo/drawbar/blob/master/crates/toshokan/spec/protocol.md#runs)
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
- A browser's rename can replace a file another program makes at the
  destination between toshokan's check that it is free and the move.
- Where a browser cannot rename files in a picked folder, every move copies the
  file and removes the old one
  ([What is supported](../getting-started/support.md#the-library-in-the-browser)).
- A folder handle can be kept only in IndexedDB, and it comes back without its
  permission. The browser asks again when the folder is chosen in **File ▸ Open
  recent**.
- The first write asks the browser to keep the storage
  (`navigator.storage.persist()`). Where the browser gives the page no storage,
  the library lasts until the tab closes.
- **Export library** is a ZIP of every file of the folder, `.drawbar/library/`
  among them, but not `.drawbar/local/` or Chromium's `.crswap` files
  (`store/copy.rs`, `store/web.rs`). **Import library…** runs in the worker before the library
  opens, refuses unless the folder holds no file outside `.drawbar/local/`, and
  takes back what it wrote if any entry fails. The library then opens as a new
  writer over the copied history, as it would over a restored backup.

[Browsers](https://github.com/jmoo/drawbar/blob/master/crates/toshokan/spec/protocol.md#browsers)
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

A WAV is known from its name alone: its row needs nothing from its contents,
and it never matches a slot. So drawbar reads one only when it is opened, acted
on or built from, never for its row, a selection or the background. toshokan
reads a WAV whole for its identity when it is first tagged. When a tagged file
has gone missing, toshokan reads every file of the same length whose identity
it does not know yet, to find where it moved.

A piano library or sample instrument is never held whole. It rests in its file,
indexed and read by range ([Large libraries](file-formats.md#large-libraries)).
Its checksum is checked in one streaming pass once something needs the verdict,
as opening or sending it does. A send reads it a transfer chunk at a time.

The assets hold at most 1 GiB of a library's files whole, and a resting file
counts for nothing. When a read would not fit, drawbar lets go of clean assets,
least recently needed first. An asset let go of keeps a summary of its read, so
its row still draws as it did and still matches its slot. A view, an unsaved
asset, and one open, selected or waiting to be sent are never let go of. A read
that cannot fit even then is tried again once room can be made, except a WAV's.
Any asset whose read failed is read again when it is next opened or acted on,
and a WAV also when a project that plays it is built.
