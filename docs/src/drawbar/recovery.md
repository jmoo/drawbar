# Recover files and edits

Start with the activity log. Click the status line at the bottom of the window
and choose **Problems** to find the file, slot or folder involved. Then find
what you see below.

| What you see | Where to look |
|---|---|
| An edit is gone, or drawbar asks which version to use | [An edit was lost or two copies disagree](#an-edit-was-lost-or-two-copies-disagree) |
| A file is missing, or has lost its tags | [A file is missing or has lost its tags](#a-file-is-missing-or-has-lost-its-tags) |
| The library is read-only or won't open | [The library is read-only or won't open](#the-library-is-read-only-or-wont-open) |
| drawbar asks whether to finish or roll back a change | [A change was interrupted](#a-change-was-interrupted) |
| **Kept apart** in the sidebar | [Kept apart](#kept-apart) |
| Sounds you don't recognize, from an earlier version | [Sounds from an earlier version](#sounds-from-an-earlier-version) |

## An edit was lost or two copies disagree

drawbar keeps a draft of each unsaved edit every few seconds, even in a
read-only library. To get an edit back:

- reopen the same library on the same computer, or
- for an edit made in an instrument slot, reconnect the same instrument and open
  that slot.

After a crash, the last few seconds may be missing. If your browser asks whether
to leave after an edit, stay a moment so drawbar can finish keeping it.

When a file on disk or a slot has changed since you edited it, drawbar asks
which version to use:

| Choice | What happens |
|---|---|
| **Keep mine** | Keeps your edits. Your next save can replace the changed copy. |
| **Take theirs** | Uses the other version and drops your edits. |
| **Keep both** | Keeps your edits as a separate file on this computer, so you can compare before deciding. |

<details>
<summary>An older draft found as a library opens</summary>

Read the names in the question. If drawbar finds an older draft as it opens a
library, **Take theirs** restores that draft and **Keep both** puts the older
draft in a separate file.

Windows or tabs using the same library on this computer can see one another's
drafts. If both kept drafts for the same sound, the most recently kept one
appears.

</details>

## A file is missing or has lost its tags

1. If you moved the file in Finder or File Explorer, return to the drawbar
   window. drawbar looks again and usually follows the move, with the file's
   tags.
2. If the library is syncing or being restored, wait for the files to arrive
   before you remove anything.
3. A missing sound with edits can be saved to bring the file back.
4. A missing file with tags may show as lost after a restart. If you meant to
   remove it, let the entry go.

<details>
<summary>Two identical files, or a folder that is not fully listed</summary>

If drawbar finds two identical candidates for a moved file, it keeps the tags
with the missing entry. Check which copy you want, then move it back to its old
location or remove the extra copy.

If a folder isn't fully listed, check the activity log for the unreadable path.
A hidden file can also keep a folder from disappearing after a move. Use **View ▸
Show hidden files** or look at the folder outside drawbar. If Windows rejects a
name ending in a space or dot, rename it on the computer that made it.

</details>

## The library is read-only or won't open

Hover over the library name for the reason. Export any work you want to keep
before you close. In a read-only library, new sounds, moves and deletions last
only until you quit. If the library never opened, work in that window is
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
change left unfinished by another copy of drawbar, it may ask what to do:

| Choice | What happens |
|---|---|
| **Finish** | Completes the change. |
| **Roll back** | Restores the files as they were before it started. |
| **Later** | Asks again next time. |
| **Leave as is** | Stops asking about that change. |

**Edit ▸ Undo** cannot take back **Finish** or **Roll back**.

### A write to the instrument was interrupted

Look in the activity log and in your library for a file named after the slot. It
holds the sound drawbar copied from the slot before replacing it, and may be the
only copy. If the library can't take it yet, choose **Show the file** to find
it, or **Later** to keep it for a writable library. **Discard** deletes the
recovery copy.

## Kept apart

**Kept apart** in the sidebar means drawbar found changes it couldn't apply to
the shared library. It can appear:

- after restoring a backup,
- while a sync is still bringing files over, or
- when drawbar's own data was copied to a second computer or a virtual machine
  and both copies went on making changes.

Let any sync finish first. Changes that were waiting for earlier ones can then
appear on their own. For what remains:

1. Open **Kept apart** to see who made each group of changes, when, and which
   sounds are involved.
2. Choose **Take** to apply one change, or **Take all** to apply the group,
   including changes beyond those listed on screen. Each change you take is a
   step you can undo.
3. If a file needs attention first, **Show file** helps you find it.

**Dismiss…** hides a group on every computer sharing the library. Dismissing
cannot be undone.

<details>
<summary>Taking a change where a sound already is, or one still waiting</summary>

If there's already a sound at that file location, taking the change applies it
there. A group that's still waiting for files to sync can only be taken or left
to wait.

</details>

## Sounds from an earlier version

drawbar brings files, folders, tags and drafts from version 0.10 and earlier into
the first library it can write. The activity log says how many sounds came in.
**Edit ▸ Undo** does not take this back, so delete the sounds you don't want.

<details>
<summary>Renamed files, unusual extensions and drafts beside sounds</summary>

Names get a number if necessary. Look under **View ▸ Show hidden files** for
unrecognized files or notes with unusual extensions. If storage was full, a
draft may appear as a separate file beside its sound.

</details>

Still missing something? [Report a problem from drawbar](help.md#report-a-problem).
