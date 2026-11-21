# Back up or move your library

Your library is everything under **This computer**. Choose the copy that suits
what you want to keep.

| What you want to keep | How to keep it |
|---|---|
| All your library files, folders and tags | Back up the library folder, or export your browser library as a ZIP |
| A single sound in its current edited form | Open the sound and choose **Export…** |
| A program or song with the pianos and samples it plays | [Export a bundle](#bundles) |
| Everything on the keyboard | Make a full backup in Nord Sound Manager |

## Where your library lives

**Desktop.** Choose **File ▸ Show library folder** to see the files. Back up the
whole folder, including the hidden `.drawbar` folder, which holds your tags and
remembers which instrument slots the sounds came from. The default locations are:

| System | Library folder |
|---|---|
| macOS | `~/Library/Application Support/drawbar/library` |
| Windows | `%LOCALAPPDATA%\drawbar\library` |
| Linux | `~/.local/share/drawbar/library` |

**Browser.** By default, your library lives in the browser you used to open
drawbar.app. Clearing the site's data deletes it, and the browser can reclaim
the space. Save a copy before changing browsers or clearing storage. Browser
retention varies. [Check your browser](../getting-started/support.md#the-library-in-the-browser).

**Unsaved edits** stay with this copy of drawbar. Save the library files you
want to take with you before you copy the folder or export the whole library.

## Keeping a copy in the browser

1. Save any library files whose names have a `*` and whose edits you want to keep.
2. Choose **File ▸ Export library**. Your browser downloads a dated ZIP with
   your files, folders and tags. Leave the library as it is until the download
   finishes.
3. Store the ZIP somewhere you can reach even if the browser's storage is cleared.

To restore, open drawbar in the destination browser. Its library must be empty,
including hidden files. Close other drawbar tabs in that browser, choose **File
▸ Import library…**, and select the ZIP drawbar exported. If the restore stops
partway and the library opens read-only, repeat the import with the same ZIP.

You can also unzip the backup and open the resulting folder on the desktop or
in a browser with **Open folder…**. A web app added to the Dock or Home Screen
has storage separate from Safari. Export from Safari and import in the web app
to take your library across.

## Opening another folder

Choose **File ▸ Open folder…** and select the folder you want to use. Its
files become **This computer**. A save now writes to the file in that folder.
This works well for a Sample Editor project whose WAVs are already arranged in
subfolders, or for a library in Dropbox, OneDrive, iCloud Drive or a network
share.

Use **File ▸ Open recent** to return to a library. On the desktop, drawbar
reopens your last library at startup. In a browser, grant access when asked.
If you granted access for one visit, drawbar starts in browser storage next time.
Pick your folder from **Open recent** to grant access again. [Browsers that can open a folder](../getting-started/support.md#the-library-in-the-browser).

Keep the folder in the same place to pick up unsaved edits next time.
In a browser, reopen it from **Open recent**. If you want to move or rename
the folder, save your edits first.

In the browser, and on a desktop disk that cannot refuse to move a file onto one
already there, drawbar checks that a name is free before a saved, copied or
moved file takes it. A file another program makes at that name in the instant
between is replaced, and what it held is lost.

## Share a folder between computers

Open the same synced folder on each computer. Sync the hidden `.drawbar` folder
along with the sounds so tags and slot information travel with them. Unsaved
edits stay on the computer where you made them. Return to the drawbar window to
see changes from another computer after sync finishes.

If you add different tags to the same sound while offline, both sets of tags
appear after sync. If something looks missing during a sync, let it finish.
For conflicting edits or a **Kept apart** message, see [Recover files and edits](recovery.md).

## Bundles

For a set list, the song file holds the locations of its programs. A program in
turn can depend on a piano or sample. A Nord Sound Manager *bundle* carries the
related files together.

1. Connect the instrument that holds the sounds. Select the programs or songs
   you want, on the instrument, on this computer, or across both.
2. Choose **Export as bundle…**. drawbar copies sounds it needs from the
   instrument to this computer and includes the dependencies it can identify.
   In a browser, look for the bundle in your downloads.
3. Click the status line to open the activity log. Check for anything left out
   before relying on the bundle elsewhere. A program file identifies its piano
   and sample by number. Copying it from its slot lets drawbar ask the instrument
   for their names.

To unpack a bundle, drop it on the window or choose **File ▸ Import…**.
The contents appear in a new folder named after the bundle. Keep the original
bundle if you need it again. [Supported bundle formats](../getting-started/support.md#files).

## Hidden files

To see other files already in the folder, such as a PDF set list, choose
**View ▸ Show hidden files**. They appear faintly. Names beginning with a dot
stay hidden. Files drawbar doesn't open can stay beside your sounds. An import
skips those files and records the reason in the activity log.

Missing a file, an unsaved edit, or a sound from an earlier version?
See [Recover files and edits](recovery.md).
