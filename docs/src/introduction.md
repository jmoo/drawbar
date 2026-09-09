# drawbar

> ⚠️ **Use at your own risk. This is alpha software.**

Drawbar is a blackbox Clavia / Nord reverse engineering project in Rust. It reads
and writes Nord keyboard files and talks to Nord instruments over USB, on Linux,
macOS, Windows, and in the browser.

| Tool | What it is |
|---|---|
| **drawbar** | A cross-platform app — a file browser over the sounds on your computer and on an attached instrument, with an editor for each. Desktop or browser tab. |
| **nord-cli** | The `nord` command — inspect, verify and edit files, and drive an attached instrument from a terminal. |

Both sit on two libraries: `nord-format` owns the bytes of a file, and `nord-usb`
owns getting those bytes on and off the instrument.

- [Run drawbar in your browser](../)
- [Source on GitHub](https://github.com/jmoo/drawbar)

Protocols and formats are decoded by interaction with real Nord devices, not by
decompiling Clavia software. Hardware validation has focused on the Electro 5.
Round-trip tests establish that file bytes are preserved; they do not establish
that every decoded parameter or newly encoded sound behaves correctly on
hardware. Back up your instrument before pointing anything here at sounds you
cannot re-create.

## Disclaimer

Not affiliated with, authorized, or endorsed by
[Clavia DMI AB](https://www.nordkeyboards.com). "Nord", "Clavia", and "Electro"
are trademarks of Clavia DMI AB, used here only to identify the hardware these
formats come from. Proprietary sound libraries and firmware are not distributed
here.
