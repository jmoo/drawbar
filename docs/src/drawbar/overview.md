# The window

drawbar is one window over two places: the files on your computer and the sounds
on your instrument. Open it at [drawbar.app](https://drawbar.app/), or run the
desktop app.

| Part | What it is for |
|---|---|
| **Top bar** | Open, New and Save, the search box, and the instrument: **Send**, the MIDI controllers, and **Connect instrument…** or the instrument's name. |
| **Browser** (left) | **Places** lists this computer and, once connected, the instrument's folders. **Kinds** and **Tags** narrow the list. |
| **Library** (center) | One table over both places. Sort by any column. |
| **Documents** (center) | Every sound you open gets a tab beside the Library. |
| **Keyboard** (center) | The instrument's folders drawn as banks of slots, once connected. |
| **Inspector** (right) | What is selected and the actions on it, and while connected, how full each folder is. |
| **Status line** | The last thing that happened, and how many problems the log holds. Click either to open the activity log. |

The search box filters the Library by name; ⌘K (Ctrl+K) puts the cursor in it.
The buttons at the two ends of the row of tabs show and hide the browser and the
inspector. A hidden panel takes no room. Drag a panel's inner edge to resize it.
drawbar keeps the layout between sessions.

## Menus

The menus are where each system keeps them:

- **macOS**: in the menu bar at the top of the screen. **drawbar ▸ About drawbar**
  and **Quit drawbar** are in the app menu.
- **Windows**: at the left of the top bar, beside the logo. Open, New and Save sit
  at the top of the browser, and Alt+F4 quits.
- **Linux**: behind the ☰ button at the right of the top bar, or F10. On GNOME,
  drawbar draws its own window buttons in the top bar.
- **Browser**: behind the ☰ button at the right of the top bar. The browser keeps
  some keys for its tabs, so Close tab and the View keys use ⌥ (⌃ on a Mac) in
  place of ⌘.

Each menu item shows its key, written the way your system writes it. In a window
too short for the whole ☰ menu, each menu becomes a submenu of it.

## The activity log

Click the status line, or press ⌥⌘L, for the log, newest first. **Problems**
shows only what went wrong. The copy button puts the whole log on the clipboard
for a bug report, and **Clear** empties it. Escape or a click elsewhere closes it.

## Selecting and acting

Click a row to select it, ⌘-click to add more, ⇧-click to select a run, and
double-click to open. Right-click for a menu that acts on everything selected.
The inspector's **Selection** card says where the selection would be sent and
offers the same actions as buttons. F2 renames.

## Reading the Library

The **Kind** column adds the instrument family, as in `Stage 4 program`, when
the list holds more than one family or a sound is not for the connected
instrument. It adds the generation, as in `v3 sample`, when the list holds sample
instruments of more than one generation.

The **Where** column says where a sound lives: on this computer, on the keyboard,
or both, with `=` when the two copies match and `≠` when they differ. Its pill
turns red when a send is waiting and yellow when the two copies differ. A name in
italics with a `*` has unsaved edits. The dot at the end of a row is green when
the slot holds what you saved, yellow when it holds something else or a send is
waiting, and gray while that is still unknown. Hover any of them for an
explanation.

## Folders and tags

Folders group sounds on this computer; the instrument never sees them. Make one
with **New ▸ New folder**. Tags label sounds without moving them, and a sound can
carry several. **Tag ▸ New tag…** in a row's menu, or **New tag** in the
inspector's Tags card, puts a new tag on the selected sounds and opens its name for
typing. Click a tag in that card to put it on, or take it off, everything
selected. Removing a folder or a tag deletes no sounds.

## MIDI controllers

**Instrument ▸ Listen to MIDI controllers** turns on every MIDI input this
computer has. [What is supported](../getting-started/support.md#midi-controllers)
says which browsers can do this. While it is on, a chip in the top bar names the
controller, or says how many there are. Its cable turns yellow while the browser
asks for access, when there is no input, or when another program is holding one,
and red when listening failed. Hover it for the details. The activity log says why a failure happened.

The keys play the key map of the sample or piano in the front tab, as clicks on
it would. Hold a chord and each key sounds until you let it go. Up to sixteen keys
sound at once, and a seventeenth stops the oldest. A click on the keyboard sounds
one key at a time. With the Library or the Keyboard tab in front, played keys are
ignored.

Nothing is sent to your instrument: a controller plays drawbar's own audition.
The desktop app listens again the next time it opens. The browser asks for access
each visit, so turn it on again there. If another program is using a controller,
close that program and turn listening off and on again.

## Theme

The sun and moon button in the top bar cycles between following your system,
light, and dark. Hover it to see which one is set.
