# Overview

drawbar is an [egui](https://github.com/emilk/egui) app over `nord-format` and
`nord-usb`: everything `nord-cli` can do that is worth a window, in a desktop
window or a browser tab. It opens at <https://drawbar.app/>.

`nord-format` owns the bytes of a file, and `nord-usb` owns getting those bytes on
and off the instrument.

## The window

| Region | What it holds |
|---|---|
| **Title bar** | The **File**, **View**, **Instrument** and **Help** menus. While an instrument is attached, its product name and a green dot. At the right, the theme button. |
| **Toolbar** | Open files, the **New** menu and Save. While an instrument is attached, **Read** and **Send** (**Send 3** when three sounds are waiting). The **Search…** box. At the right, the three dock toggles. |
| **Browser** (left dock) | The **PLACES**, **KINDS** and **TAGS** sections. |
| **Centre** | The **Library** tab, the **Keyboard** tab while an instrument is attached, and one tab per open document. The **+** after the last tab is the New menu. |
| **Inspector** (right dock) | **SELECTION**, and while an instrument is attached, **INSTRUMENT** with **ROOM** and **INFO**. |
| **Bottom dock** | The **SEND QUEUE** page while an instrument is attached, and the **ACTIVITY LOG** page. |
| **Status bar** | What just happened, or a spinner while something runs. Click it to open the activity log. At the right, how full the Samples and Programs folders are. |

The instrument controls do not appear until an instrument answers: Read, Send,
the send queue, the Keyboard tab, INSTRUMENT, and every Instrument menu item except
**Connect…**.

A side dock collapses to a narrow rail and opens again from the glyph on it; the
bottom dock collapses to its header. Drag a dock's inner edge to resize it. **View** toggles **Browser panel** (⌥⌘B),
**Inspector panel** (⌥⌘I) and **Bottom dock** (⌥⌘L). Which docks are open, their
sizes and the bottom dock's page are kept between sessions. Clicking the title of
the page already showing collapses the bottom dock.

In a browser tab, ⌘W, ⌘Q, ⌘2 and ⌘3 belong to the browser, so **Close tab**,
**Keyboard** and **Document** work only from their menus, and there is no **Quit**.

## The browser

**PLACES** holds:

- **This computer**, with your folders and everything not in a folder. See
  [Files on this computer](this-computer.md).
- The attached instrument, with one row per folder the instrument declares and the
  banks and slots under each. Until one is attached, the row reads **Connect an
  instrument…**. See [The instrument](instrument.md).
- **Waiting to send** and **Differs**, when either counts something.

**KINDS** lists each kind of thing present on this computer or the attached
instrument, such as Programs, Samples or Sample Editor projects. It is hidden when
there is only one kind. **TAGS** lists your tags, and **new tag** makes one.

Clicking a place, kind, tag or state narrows the Library and brings it forward.
Clicking the instrument's own row opens the Keyboard tab instead; clicking one of
its folders opens the Keyboard tab on that folder.

Click a row to pick it, ⌘-click to add or remove one, and ⇧-click to pick a run.
Escape, or a click below the last row, lets go. Double-click a row to open it.
Right-click a row for its menu; a menu acts on everything picked. **F2**, or
**Rename** in the menu, renames the one row picked.

Names appear without their format extension. Hover a name for the full name.

### Folders and tags

A folder groups the list on this computer; the instrument never sees one. Make one
with **New ▸ New folder**, then drag sounds into it or use **Move to folder**.
**Remove folder** returns what was in it to the list and deletes nothing.

A tag labels sounds without moving them, and one sound can wear several. Use
**Tag** in a row's menu, or **Save as gig…** to put everything picked under a new
tag. **Remove tag** takes the tag off everything wearing it and deletes nothing.

## The Library

The Library tab is always open, is always the first tab, and cannot be closed.
Closing the last document returns to it. It is one table over this computer and
the attached instrument, narrowed by the browser's rows and by the **Search…** box.
Typing in the box brings the Library forward.

Its columns are **kind**, **name**, **tags**, **where**, **at**, **size** and
**needs**. Click a column head to sort by it. **where** reads:

| Cell | Means |
|---|---|
| `both =` | On this computer and in a slot holding the same bytes. |
| `both ≠` | On this computer and in a slot it was matched to, and the two differ. |
| `both` | On this computer and in a slot matched by name; nothing can yet say whether they agree. |
| `computer` | On this computer only. Hover it: "not for this keyboard" means the attached instrument does not take it. |
| `keyboard` | On the instrument only. |

A name in italics with a `*` has been edited since it was last saved. The dot at a
row's right end is green when the slot holds what was saved, yellow when it holds
something else or a write is waiting, and grey when the match is not known yet.
Every dot explains itself on hover.

Tick the checkboxes to act on several rows at once. The footer then shows **N
selected**, **clear**, what sending them would do, **Review send queue**, and
**Queue for sending**, **Copy to this computer**, **Export…**, **Tag…** and
**Delete…**. An action that cannot apply is greyed, and its hover says why.

## The Keyboard tab

One tab for the attached instrument. The band at the top names it, says how long
ago the folder was read, and offers **Read again**. Under it, a row of folders
switches what the tab shows.

Programs and Live are drawn as a bank of slots, five to a row, with bank chips
above. Other folders are lists with their slot, name, size and one fact: what a set
list **plays**, what a sample is **played by**, or a piano's **category**. Pianos
and Samples show how much room is left under the list. Double-click a slot to open
it, and right-click it for its menu.

## The inspector

**SELECTION** describes what is picked: its name, kind, where it is and its size,
the tags it wears as chips, and the dependencies the instrument reported for a
picked slot. Click a solid tag chip to take that tag off everything picked, and a
hollow one to put it on all of them.

**INSTRUMENT** appears while an instrument is attached. **ROOM** has a meter for
each folder that can fill, with the space queued sends will take. **INFO** lists
the instrument's USB facts. Both collapse, and stay as they were left.

## The bottom dock

**SEND QUEUE** is where sends wait for you to review them; see
[The instrument](instrument.md#the-send-queue). **ACTIVITY LOG** keeps everything
the app has reported, protocol detail included. **Clear** empties it, and
**Help ▸ Copy activity log** puts it on the clipboard.

## Theme

The theme button in the title bar reads **Theme: auto**, **Theme: light** or
**Theme: dark**. Auto follows the desktop's or the browser's own setting as it
changes. Clicking the button cycles through the three; **View ▸ Theme** picks one
directly. The choice is kept between sessions.

## The rest of this section

- [Files on this computer](this-computer.md): opening, what is kept, exporting,
  and names.
- [The instrument](instrument.md): connecting, reading, moving things, the send
  queue, and safety.
- [Editing](editing.md): the document header, its faces, and the field document.
- [Samples](samples.md): the editor for `.nsmp` instruments and `.nsmpproj`
  projects.
- [Pianos](pianos.md): the `.npno` piano library editor.
- [Help and About](help.md): the guide, What's new, and the licences.
- [Build and run](build.md): native and web builds, and browser support.
