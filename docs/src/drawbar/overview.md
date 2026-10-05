# The window

drawbar is one window over two places: the files on your computer and the sounds
on your instrument. Open it at [drawbar.app](https://drawbar.app/), or run the
desktop app.

| Part | What it is for |
|---|---|
| **Top bar** | Open, New and Save, the search box, **Send**, and **Connect instrument…** or your instrument's name. |
| **Browser** (left) | **This computer** and, once connected, the instrument's folders. **Kinds** and **Tags** narrow the list. |
| **Library** (center) | One table over both places. Sort by any column. In a narrow window, other columns hide before the names shorten. |
| **Documents** (center) | Every sound you open gets a tab. |
| **Keyboard** (center) | The instrument's slots, bank by bank, once connected. |
| **Inspector** (right) | What is selected and what you can do with it. |
| **Status line** | The last thing that happened. Click it for the activity log. |

⌘K (Ctrl+K) jumps to the search box. The buttons at the two ends of the row of
tabs show and hide the side panels, and dragging a panel's edge resizes it.

## Menus

On macOS the menus are in the menu bar. On Windows they are at the left of the
top bar. On Linux and in the browser they are behind the ☰ button. In the
browser, a few keys use ⌥ (⌃ on a Mac) in place of ⌘.

## The activity log

Click the status line, or press ⌥⌘L, for the log. **Problems** shows only what
went wrong. The copy button puts the whole log on the clipboard for a bug report.

## Selecting

Click a row to select it, ⌘-click to add more, ⇧-click to select a run, and
double-click to open. Right-click for a menu that acts on everything selected.
F2 renames. With more than 64 rows selected, the inspector sums them up: how
many, how large, and how many of each kind.

## Reading the Library

The **Where** column says where a sound lives: on this computer, on the keyboard,
or both, with `=` when the two copies match and `≠` when they differ. It turns
red when a send is waiting. A name in italics with a `*` has unsaved edits. Hover
anything for an explanation.

## Folders and tags

Folders on this computer are real folders, and the instrument never sees them.
Make one with **New ▸ New folder**. Removing a folder moves what was in it up a
level.

Tags label sounds without moving them, and a sound can have several. **Tag ▸ New
tag…** in a row's menu tags the selected sounds. Click a tag in the inspector to
put it on, or take it off, everything selected. Removing a tag deletes no sounds.

## MIDI controllers

**Instrument ▸ Listen to MIDI controllers** lets a controller play the sample or
piano in the front tab. Nothing is sent to your instrument.
[What is supported](../getting-started/support.md#midi-controllers) says which
browsers can do this. The browser asks for access on each visit, so turn it on
again there. If another program is using the controller, close it and turn
listening off and on again.

## Theme and zoom

The sun and moon button in the top bar switches between your system's theme,
light and dark. The magnifier at the right of the status line zooms the window.
