# The window

drawbar is one window over two places: the files on your computer and the sounds
on your instrument. Open it at [drawbar.app](https://drawbar.app/), or run the
desktop app.

| Part | What it is for |
|---|---|
| **Browser** (left) | **PLACES** lists this computer and, once connected, the instrument's folders. **KINDS** and **TAGS** narrow the list. |
| **Library** (centre) | One table over both places, with a **Search…** box. Sort by any column. |
| **Documents** (centre) | Every sound you open gets a tab beside the Library. |
| **Keyboard** (centre) | The instrument's folders drawn as banks of slots, once connected. |
| **Inspector** (right) | What is picked, and while connected, how full each folder is. |
| **Bottom dock** | The **send queue** and the **activity log**. |
| **Status bar** | The last thing that happened. Click it to open the log. |

The toolbar holds Open, New and Save, and once connected, **Read** and **Send**.
Each dock collapses and resizes, and the layout is kept between sessions.

## Picking and acting

Click a row to pick it, ⌘-click to add more, ⇧-click to pick a run, and
double-click to open. Right-click for a menu that acts on everything picked. In the
Library, tick the checkboxes to act on many rows at once from the footer. F2
renames.

## Reading the Library

The **where** column says where a sound lives: on this computer, on the keyboard,
or both, with `=` when the two copies match and `≠` when they differ. A name in
italics with a `*` has unsaved edits. The dot at the end of a row is green when
the slot holds what you saved, yellow when it holds something else or a send is
waiting, and grey while that is still unknown. Hover any of them for an
explanation.

## Folders and tags

Folders group sounds on this computer; the instrument never sees them. Tags label
sounds without moving them, and a sound can carry several. Both come from a row's
menu or the **New** menu. Removing a folder or a tag deletes no sounds.

## MIDI controllers

**Instrument ▸ Listen to MIDI controllers** turns on every MIDI input this
computer has. [What is supported](../getting-started/support.md#midi-controllers)
says which browsers can. While it is on, the top right of the window names the
controller you are playing, or how many there are, with a green light. The light
is yellow while the browser asks for access, when there is no input, or when
another program is holding one. It is red when listening failed. Hover it for the details, and the activity log says why a
failure happened.

The keys play the key map of the sample or piano open in front, as clicks on it
would. Hold a chord and each key sounds until you let it go. Up to sixteen keys
sound at once, and a seventeenth stops the oldest. A click on the keyboard sounds
one key at a time. With the Library or the Keyboard tab in front, played keys are
ignored.

Nothing is sent to your instrument: a controller plays drawbar's own audition.
The desktop app listens again the next time it opens. The browser asks for access
each visit, so turn it on again there. If another program is using a controller,
close that program and turn listening off and on again.

## Theme

The button at the top right cycles between following your system, light, and
dark.
