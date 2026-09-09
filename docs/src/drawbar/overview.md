# Overview

drawbar is an [egui](https://github.com/emilk/egui) app over `nord-format` and
`nord-usb` — everything `nord-cli` can do that is worth a window, reachable from
a browser tab or a desktop one.

The window is a file browser over the places your sounds live:

| Region | What it holds |
|---|---|
| **Storage** (left) | **This computer** — dropped or opened files, copies pulled off the instrument, fresh defaults. **Connect instrument** sits beside Open… and New; attached, the instrument takes a column of its own alongside, so moving a sound between the two is a short drag. |
| **Tabs** (centre) | One document per thing you opened. Double-click anything in the sidebar to open it. Each document has a **Basic** and an **Advanced** face. |
| **Status strip** (bottom) | One line about what just happened, and a spinner while something is running. Click it for the full activity log. |

The divider between the storage columns is dragged to give one more room than the
other; double-click it for an even split. **Light or dark follows the machine**
until the button at the top right is clicked, which cycles to a held light, a
held dark, and back to auto. Both choices are kept between sessions.

The full tour is in
[`crates/drawbar/README.md`](https://github.com/jmoo/drawbar/blob/master/crates/drawbar/README.md).
