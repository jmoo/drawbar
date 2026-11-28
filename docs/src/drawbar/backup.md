# Back up or move your library

Your library is everything under **This computer**. This page says where it
lives and how to keep a copy you can restore.

| What you want to keep | How to keep it |
|---|---|
| All your library files, folders and tags | Back up the [library folder](#where-your-library-lives), or [export your browser library](#keeping-a-copy-in-the-browser) as a ZIP |
| A single sound in its current edited form | Open the sound and choose **Export…** |
| A program or song with the pianos and samples it plays | [Export a bundle](bundles.md) |
| Everything on the keyboard | Make a full backup in Nord Sound Manager |

## Where your library lives

**On the desktop**, choose **File ▸ Show library folder** to see the files. Back
up the whole folder, including the hidden `.drawbar` folder. It holds your tags
and remembers which instrument slots the sounds came from, so it must travel
with the library. The default locations are:

| System | Library folder |
|---|---|
| macOS | `~/Library/Application Support/drawbar/library` |
| Windows | `%LOCALAPPDATA%\drawbar\library` |
| Linux | `~/.local/share/drawbar/library` |

**In a browser**, your library lives by default in the browser you used to open
drawbar.app. Clearing the site's data deletes it, and the browser can reclaim
the space. Save a copy before you change browsers or clear storage. Browser
retention varies; [check your browser](../getting-started/support.md#the-library-in-the-browser).

**Unsaved edits** stay with this copy of drawbar. Save the files you want to
take with you before you copy the folder or export the library.

## Keeping a copy in the browser

1. Save the library files whose names show a `*` and whose edits you want to keep.
2. Choose **File ▸ Export library**. Your browser downloads a dated ZIP with your
   files, folders and tags. Leave the library as it is until the download
   finishes.
3. Store the ZIP somewhere you can reach even if the browser's storage is cleared.

## Restoring a copy

1. Open drawbar in the browser you are restoring to. Its library must be empty,
   including hidden files.
2. Close other drawbar tabs in that browser.
3. Choose **File ▸ Import library…** and select the ZIP drawbar exported.

If the restore stops partway and the library opens read-only, import the same
ZIP again.

You can also unzip the backup and open the folder with
[**Open folder…**](folders.md#opening-another-folder), on the desktop or in a
browser.

<details>
<summary>Moving to a web app on the Dock or Home Screen</summary>

A web app added to the Dock or Home Screen has storage separate from Safari.
Export from Safari and import in the web app to take your library across.

</details>

Missing a file, an unsaved edit, or a sound from an earlier version? See
[Recover](recovery.md).
