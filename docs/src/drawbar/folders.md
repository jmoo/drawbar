# Folders and sync

drawbar can use any folder as your library. Open a folder you already have, or
keep your library in a folder that a sync service shares between your computers.

## Opening another folder

1. Choose **File ▸ Open folder…**.
2. Select the folder. Its files become **This computer**, and a save writes to
   the file in that folder.

This suits a Sample Editor project whose WAVs are already arranged in
subfolders, or a library kept in Dropbox, OneDrive, iCloud Drive or on a network
share.

To return to a library, choose it from **File ▸ Open recent**. On the desktop,
drawbar reopens your last library at startup. In a browser, grant access when
asked. If you granted access for one visit only, drawbar starts in browser
storage next time; pick your folder from **Open recent** to grant access again.
[Browsers that can open a folder](../getting-started/support.md#the-library-in-the-browser).

Unsaved edits are picked up next time only if the folder stays where it is.
Save your edits before you move or rename the folder.

## Share a folder between computers

A sync service such as Dropbox, OneDrive or iCloud Drive can keep one library on
several computers.

1. Put the library in a folder the service syncs. To use the library you
   already have, save your edits, then move the
   [library folder](backup.md#where-your-library-lives) there.
2. Make sure the service syncs the hidden `.drawbar` folder with the sounds. It
   holds your tags and which instrument slots the sounds came from. Without it,
   they stay behind.
3. On each computer, choose **File ▸ Open folder…** and select that same folder.

After a change on one computer, wait for the sync to finish, then return to the
drawbar window on the other to see it. Unsaved edits stay on the computer where
you made them. Different tags added to the same sound on two computers while
offline all appear once both have synced.

<details>
<summary>While a sync is still settling</summary>

If a file or tag looks missing during a sync, let the sync finish before you
remove or redo anything. If drawbar asks which version of an edit to keep, or
shows **Kept apart** in the sidebar, see [Recover](recovery.md).

Open the synced folder on each computer, and leave drawbar's own data where
each computer keeps it. If that data is copied to a second computer or virtual
machine and both copies go on making changes,
[Kept apart](recovery.md#kept-apart) can appear.

</details>

<details>
<summary>Another program writing in the same folder</summary>

In the browser, and on a desktop disk that cannot refuse to move a file onto one
already there, drawbar checks that a name is free before a saved, copied or moved
file takes it. A file another program makes at that name in the instant between
is replaced, and what it held is lost.

</details>

## Hidden files

Files drawbar doesn't open, such as a PDF set list, can stay beside your sounds.
To see them, choose **View ▸ Show hidden files**. They appear faintly. Names
beginning with a dot stay hidden. An import skips files drawbar doesn't open and
records the reason in the activity log.
