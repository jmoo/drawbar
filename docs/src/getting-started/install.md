# Install

## In the browser

Open [drawbar.app](https://drawbar.app/). Nothing to install. Your library is
kept in the browser's storage for the site, which the browser can delete. You can
[keep a copy](../drawbar/this-computer.md#keeping-a-copy-in-the-browser), or
[open a folder](../drawbar/this-computer.md#opening-another-folder) on your
computer instead.

Files work in any browser. [What is supported](support.md) says which browsers
can connect to an instrument or hear a MIDI controller.

## On the desktop

On Windows, download `drawbar-<version>-x86_64-windows.exe` from the newest
[drawbar release](https://github.com/jmoo/drawbar/releases)
and run it. It is not signed, so Windows may ask before running it the first
time.

On a Mac with Apple silicon and macOS 14 or later, download
`drawbar-<version>-aarch64-macos.dmg` from the newest
[drawbar release](https://github.com/jmoo/drawbar/releases), open it, and drag
drawbar to Applications. It is not signed, so macOS refuses to open it the first
time. Choose **Open Anyway** in System Settings ▸ Privacy & Security, then open
drawbar again.

On Linux, download `drawbar-<version>-x86_64-linux.AppImage` from the newest
[drawbar release](https://github.com/jmoo/drawbar/releases), make it executable,
and run it:

```sh
chmod +x drawbar-*-x86_64-linux.AppImage
./drawbar-*-x86_64-linux.AppImage
```

If it says FUSE is missing, add `--appimage-extract-and-run` to the second line.
drawbar brings its own graphics driver, Mesa, which runs Intel and AMD graphics.
On other graphics, NVIDIA's driver among them, it draws without the graphics card,
which works but uses more of the processor. To reach an instrument, Linux also
needs [a udev rule](#connecting-on-linux).

Elsewhere, with [Nix](https://nixos.org/download/) and flakes enabled:

```sh
nix run github:jmoo/drawbar#drawbar
```

Or build it with Cargo. On Linux this needs the ALSA headers and `pkg-config`.

```sh
cargo install drawbar
```

## The `nord` command

On Linux, download `nord-<version>-x86_64-linux.tar.gz`, or
`nord-<version>-aarch64-linux.tar.gz` on ARM, from the newest
[nord-cli release](https://github.com/jmoo/drawbar/releases), unpack it, and put
`nord` in a folder on your `PATH`. It is statically linked, so it needs no other
libraries.

On Windows, download `nord-<version>-x86_64-windows.exe` from the same release,
rename it to `nord.exe`, and put it in a folder on your `PATH`.

```sh
cargo install nord-cli                           # installs `nord`
nix run github:jmoo/drawbar#nord-cli -- --help   # or run it with Nix
```

To build either from a source checkout, see
[Building from source](../reference/building.md).

## Connecting on Linux

Linux lets drawbar and `nord` open an instrument only once a udev rule allows it.
This rule allows whoever is signed in at the computer:

```sh
echo 'SUBSYSTEM=="usb", ATTR{idVendor}=="0ffc", TAG+="uaccess"' |
  sudo tee /etc/udev/rules.d/70-nord.rules
sudo udevadm control --reload
```

Then unplug the instrument and plug it in again.

## If drawbar does not load

The loading page says which step failed and offers to try again. The usual
causes:

- The connection dropped, or the download stopped partway. Reload the page.
- The browser lacks WebAssembly or WebGL2, which drawbar needs. Every current
  browser has both. If the page says WebGL isn't supported, turn on hardware
  acceleration in the browser's settings and restart the browser.
- The browser is too old to keep drawbar's library, and the page says so before
  downloading anything. [What is supported](support.md#the-library-in-the-browser)
  says which browsers can.
- An extension or a content blocker stopped drawbar. Allow this site and reload.
