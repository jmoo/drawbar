# 🎚️ drawbar

**Read, edit and move the sounds on your Nord keyboard.** In your browser, from
your terminal, or from your own code.

[**Open drawbar**](https://drawbar.app/) · [User guide](docs/src/introduction.md) · [What is supported](docs/src/getting-started/support.md)

![drawbar with an instrument attached and a piano library open in its editor](docs/src/assets/screenshot.png)

> ⚠️ **Alpha.** Back up your instrument and your files first. Everything here was
> worked out by studying real instruments, and
> [what has been tested on hardware](docs/src/getting-started/support.md) is
> written down.

## What's here

| | | |
|---|---|---|
| 🎚️ | [drawbar](crates/drawbar/README.md) | The app. Browse the sounds on your computer and your instrument side by side, edit them, and send them back. Browser or desktop. |
| 🛠️ | [nord-cli](crates/nord-cli/README.md) | `nord`, the same from a terminal, for scripts. |
| 🎹 | [nord-format](crates/nord-format/README.md) | Rust library: read and write Nord files, byte for byte. |
| 🔌 | [nord-usb](crates/nord-usb/README.md) | Rust library: talk to a Nord over USB. |
| 🧬 | [nord-bits-derive](crates/nord-bits-derive/README.md) | The macro behind nord-format's layouts. |

## Try it

Open [drawbar.app](https://drawbar.app/) in Chrome or Edge. Or, with
[Nix](https://nixos.org/download/):

```sh
nix run github:jmoo/drawbar#drawbar              # the desktop app
nix run github:jmoo/drawbar#nord-cli -- --help   # the nord command
```

The [user guide](docs/src/introduction.md) covers both. Developers will find the
file formats, the USB protocol and
[how to build from source](docs/src/reference/building.md) in its Reference
section, and the house rules in [CONTRIBUTING.md](CONTRIBUTING.md).

## Commnity

Check out other similar projects @ [Community](docs/src/community.md).

## Disclaimer

Not affiliated with, authorized, or endorsed by Clavia DMI AB
(https://www.nordkeyboards.com). "Nord", "Clavia", and "Electro" are trademarks
of Clavia DMI AB, used here only to identify the hardware these formats come
from. Committed fixtures are self-generated files and protocol captures.
Proprietary sound libraries and firmware are not distributed here.
