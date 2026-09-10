# Overview

drawbar is an [egui](https://github.com/emilk/egui) app over `nord-format` and
`nord-usb` — everything `nord-cli` can do that is worth a window, reachable from
a browser tab or a desktop one. `nord-format` owns the bytes of a file, and
`nord-usb` owns getting those bytes on and off the instrument.

The window is a file browser over the places your sounds live:

| Region | What it holds |
|---|---|
| **Storage** (left) | **This computer** — dropped or opened files, copies pulled off the instrument, fresh defaults. **Connect instrument** sits beside Open… and New; until an instrument answers, this is the whole sidebar. Attached, the **instrument** takes a column of its own alongside — folders (Programs, Set lists, Samples, Pianos, Live, Settings) filling in by themselves — so moving a sound between the two is a short drag. Each column scrolls on its own, and the divider between them is dragged to give one more room than the other; double-click it for an even split, and where it was left is kept between sessions |
| **Tabs** (centre) | one document per thing you opened. Double-click anything in the sidebar to open it; something on the instrument is copied here first. Each has a **Basic** and an **Advanced** face |
| **Status strip** (bottom) | one line about what just happened, and a spinner while something is running. Click it for the full activity log, protocol detail and all |

**Light or dark, whichever the machine is using.** The button at the top right says
which — *auto* follows the desktop's or the browser's own setting and changes with it
mid-session; clicking cycles to a held light, a held dark, and back to auto. The choice
is kept between sessions.

## The rest of this section

- [Files on this computer](this-computer.md) — what arrives, what is kept, and
  how a document and its name behave.
- [The instrument](instrument.md) — connecting, folders and slots, moving things,
  safety, and sending changes back.
- [Editing](editing.md) — the Basic and Advanced faces of a document.
- [Build and run](build.md) — native and web builds, and browser support.
