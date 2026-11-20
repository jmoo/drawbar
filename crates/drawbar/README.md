# drawbar

**Your Nord's sounds, in a window.** See what is on your computer and on your
instrument side by side, edit programs, samples and pianos, and send them back.
Runs in the browser or on the desktop.

![drawbar with an instrument attached and a piano library open in its editor](../../docs/src/assets/screenshot.png)

Open [drawbar.app](https://drawbar.app/), or run the desktop app:

```sh
nix run github:jmoo/drawbar#drawbar
cargo install drawbar
```

On Windows, download it from the newest
[drawbar release](https://github.com/jmoo/drawbar/releases).
On Linux, download the AppImage from the same release; see
[Install](../../docs/src/getting-started/install.md#on-the-desktop).

On a Mac with Apple silicon, download the disk image from the same release; see
[Install](../../docs/src/getting-started/install.md#on-the-desktop) for the first
open.

- Drop Nord files in, or connect an instrument and read what it holds.
- Edit a program on a panel laid out like the instrument's, or any field in a
  table.
- Trim a piano library until it fits, build a sample instrument from WAVs or a
  Sample Editor project, and listen to either from the screen or a MIDI
  controller before you send it.
- Turn a recording of your own instrument into a piano library: find each note
  in the take, trim and loop it, hear it as the Nord will play it, then build the
  library.
- Sync with the keyboard in one go: one sheet shows what changed on each side,
  and which way each sound goes.
- Unpack a Nord Sound Manager bundle into a folder, or export sounds from either
  side, with the pianos and samples they play, as one.
- Keep a text note beside the sounds for the set list and the cues.
- In the browser, save a copy of the library as one ZIP file, and open it again
  in another browser or after the browser cleared its storage.
- On the desktop, or in a browser that can open a folder, open any folder as the
  library, such as a Sample Editor project. Nothing in it changes until you
  change something, and you can undo what you change while it is open.
- Keep the library in Dropbox, OneDrive, iCloud Drive or on a network share, and
  use it from every computer that syncs it; see
  [Your files](../../docs/src/drawbar/this-computer.md#opening-another-folder).

Connecting an instrument from the browser needs Chrome, Edge or Brave.

Start with [The window](../../docs/src/drawbar/overview.md) in the user guide,
and read [What is supported](../../docs/src/getting-started/support.md) before
trusting alpha software with sounds you cannot re-create.
[Building from source](../../docs/src/reference/building.md) covers a checkout.

## Disclaimer

Not affiliated with, authorized, or endorsed by Clavia DMI AB. "Nord", "Clavia",
and "Electro" are trademarks of Clavia DMI AB, used here only to identify the
hardware these formats come from.
