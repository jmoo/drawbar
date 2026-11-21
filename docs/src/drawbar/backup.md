# Back up or move your library

Your library is everything under **This computer**. Choose the copy that suits
what you want to keep.

| What you want to keep | How to keep it |
|---|---|
| All your library files, folders and tags | Back up the library folder, or export your browser library as a ZIP |
| A single sound in its current edited form | Open the sound and choose **Export…** |
| A program or song with the pianos and samples it plays | [Export a bundle](bundles.md) |
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

Missing a file, an unsaved edit, or a sound from an earlier version?
See [Recover files and edits](recovery.md).
