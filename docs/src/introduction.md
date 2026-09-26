# drawbar

drawbar reads, edits and moves the sounds on a Nord keyboard. It comes as an app
that runs in your browser or on your desktop, and as `nord`, a command for your
terminal.

![drawbar with an instrument attached and a piano library open in its editor](assets/screenshot.png)

> **Alpha software.** Back up your instrument and your files before you use it.
> The file formats and the USB protocol were worked out by studying real
> instruments. [What is supported](getting-started/support.md) lists what has
> been tested on hardware.

**Open drawbar** at [drawbar.app](https://drawbar.app/) in Chrome or Edge.
[Install](getting-started/install.md) covers the desktop app and `nord`.

## In this guide

- **drawbar** is the app. Start with [The window](drawbar/overview.md).
- **nord-cli** is the `nord` command. Start with its
  [overview](nord-cli/overview.md).
- **Reference** is for developers: building from source, the file formats, the
  USB protocol, and how to contribute.

Both tools are built on two Rust libraries you can use in your own projects:
`nord-format` for files and `nord-usb` for the instrument. The source is on
[GitHub](https://github.com/jmoo/drawbar).

## Disclaimer

Not affiliated with, authorized, or endorsed by
[Clavia DMI AB](https://www.nordkeyboards.com). "Nord", "Clavia", and "Electro"
are trademarks of Clavia DMI AB, used here only to identify the hardware these
formats come from. Proprietary sound libraries and firmware are not distributed
here.
