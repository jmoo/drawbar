# Install

## In the browser

Open [drawbar.app](https://drawbar.app/). Nothing to install, and your files stay
in the browser's own storage.

Only Chrome and Edge can connect to an instrument, because Firefox and Safari do
not support WebUSB, and they are also the browsers that can hear a MIDI
controller. Files work in any browser.

Close Nord Sound Manager before connecting. It keeps the USB connection to
itself, so nothing else can reach the instrument while it is running.

## On the desktop

With [Nix](https://nixos.org/download/) and flakes enabled:

```sh
nix run github:jmoo/drawbar#drawbar
```

## The `nord` command

```sh
cargo install nord-cli                           # installs `nord`
nix run github:jmoo/drawbar#nord-cli -- --help   # or run it with Nix
```

There are no packaged downloads yet. To build either from a source checkout, see
[Building from source](../reference/building.md).
