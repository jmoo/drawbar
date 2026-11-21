# Recover files and edits

Start by clicking the status line at the bottom of the window. Choose
**Problems** in the activity log to find the file, slot or folder involved.

## An edit was lost or two copies disagree

Reopen the same library on the same computer. For an edit made directly in an
instrument slot, reconnect the same instrument and open that slot. drawbar
keeps drafts every few seconds, even in a read-only library. After a crash the
last few seconds may be missing. If your browser asks whether to leave after an
edit, stay a moment so drawbar can finish keeping it.

When a file on disk or a slot has changed since you edited it, drawbar asks
which version to use:

- **Keep mine** keeps your edits. Your next save can replace the changed copy.
- **Take theirs** uses the other version and drops your edits.
- **Keep both** keeps your edits as a separate file on this computer, so you can
  compare before deciding.

Read the names in the question. If drawbar finds an older *draft* as it opens a
library, **Take theirs** restores that draft and **Keep both** puts the older
draft in a separate file. Windows or tabs using the same library on this
computer can see one another's drafts. If both kept drafts for the same sound,
the most recently kept one appears.

## A file is missing or has lost its tags

Return to the drawbar window after moving a file in Finder or File Explorer.
drawbar looks again and usually follows the move with the file's tags. If it
finds two identical candidates, it keeps the tags with the missing entry. Move
the intended file back to its old location or remove the extra copy after
checking which you want.

A missing sound with edits can be saved to bring the file back. A missing file
with tags may show as lost after a restart. If you meant to remove it, you can
let the entry go. If the library is syncing or restoring, wait for the files to
arrive before removing anything.

If a folder isn't fully listed, check the activity log for the unreadable
path. A hidden file can also keep a folder from disappearing after a move. Use
**View ▸ Show hidden files** or inspect the folder outside drawbar. If Windows
rejects a name ending in a space or dot, rename it on the computer that made it.

## The library is read-only or won't open

Hover over the library name for the reason. Export any work you want to keep
before closing. In a read-only library, new sounds, moves and deletions can
last only until you quit. If the library never opened, work in that window is
temporary.

| Message or circumstance | What to try |
|---|---|
| No permission to write | Restore write access, then choose the folder in **File ▸ Open recent**. |
| A newer drawbar wrote the library | Open it with a current version of drawbar. |
| A network or USB drive cannot save reliably | Copy the whole folder to a local disk and open the copy. |
| Too many windows or tabs | One library opens in at most 16 windows or tabs. Close another window using that library, then reopen the library. |
| The library stopped answering | Read the list of recent edits that reopening would lose. When ready, reopen from the prompt or **File ▸ Open recent**. |
| A browser gives drawbar no storage | Export your files before closing. Use a regular browser window or the desktop app for a lasting library. Without storage, drawbar can send only to empty slots. |

## A change was interrupted

After a crash, drawbar tries to finish its own interrupted file operation. For a
change left unfinished by another copy of drawbar, it may ask what to do.
**Finish** completes that operation. **Roll back** restores the files as they
were before it started. Neither choice can be undone with **Edit ▸ Undo**.
**Later** asks again next time. **Leave as is** stops asking about that change.

If a **write to the instrument** was interrupted, look in the activity log and
your library for a file named after the slot. It holds the sound drawbar copied
before replacing it and may be the only copy. If the library can't take it yet,
choose **Show the file** to locate it or **Later** to keep it for a writable
library. **Discard** deletes the recovery copy.

## Kept apart

**Kept apart** in the sidebar means drawbar found changes it couldn't apply to
the shared library. This can happen after restoring a backup, while a sync is
still bringing files over, or when drawbar's own data was copied to a second
computer or a virtual machine and both copies went on making changes. Let sync finish first. Changes that were waiting for
earlier ones can then appear on their own.

Open **Kept apart** to see who made each group of changes, when, and which
sounds are involved. **Take** applies one change. **Take all** includes changes
beyond the ones listed on screen. Each change you take is a step you can undo.
If there's already a sound at that file location, taking the change applies it
there. **Show file** helps locate a file that needs attention first.

**Dismiss…** hides a group on every computer sharing the library. Dismissing
cannot be undone. A group that's still waiting for files to sync can only be
taken or left to wait.

## Sounds from an earlier version

drawbar brings files, folders, tags and drafts from version 0.10 and earlier
into the first library it can write. The activity log tells you how many sounds
came in. **Edit ▸ Undo** does not take this back, so delete the sounds you
don't want. Names get a number if necessary. Look under **View ▸ Show hidden
files** for unrecognized files or notes with unusual extensions. If storage
was full, a draft may appear as a separate file beside its sound.

Still missing something? [Report a problem from drawbar](help.md#report-a-problem).
