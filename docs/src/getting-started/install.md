# Install

## In the browser

Open [drawbar.app](https://drawbar.app/). Nothing to install. Your library is
kept in the browser's storage for the site, which the browser can delete; [Your
files](../drawbar/this-computer.md#where-your-library-lives) says how to keep a
copy, and how to open a folder on your computer instead.

Only Chrome and Edge can connect to an instrument. Files work in any browser.
Which browsers can play the key maps from a MIDI controller is on
[What is supported](support.md#midi-controllers).

Close Nord Sound Manager before connecting, since it keeps the instrument to
itself.

## On the desktop

With [Nix](https://nixos.org/download/) and flakes enabled:

```sh
nix run github:jmoo/drawbar#drawbar
```

Or build it with Cargo. On Linux this needs the ALSA headers and `pkg-config`.

```sh
cargo install drawbar
```

## The `nord` command

```sh
cargo install nord-cli                           # installs `nord`
nix run github:jmoo/drawbar#nord-cli -- --help   # or run it with Nix
```

There are no prebuilt downloads yet. To build either from a source checkout, see
[Building from source](../reference/building.md).

## If drawbar does not load

The loading page says which step failed and offers to try again. The usual
causes:

- The connection dropped, or the download stopped partway. Reload the page.
- The browser lacks WebAssembly or WebGL2, which drawbar needs. Every current
  browser has both. If the page says WebGL isn't supported, turn on hardware
  acceleration in the browser's settings and restart the browser.
- The browser is too old to keep drawbar's library, and the page says so before
  downloading anything. drawbar runs in a recent Chrome, Edge, Firefox or Safari;
  [What is supported](support.md#the-library-in-the-browser) has more.
- An extension or a content blocker stopped drawbar. Allow this site and reload.
