# Open or install drawbar

[Browser](#in-the-browser) · [Windows](#windows) · [macOS](#macos) ·
[Linux](#linux) · [The `nord` command](#the-nord-command)

## In the browser

[Open drawbar](https://drawbar.app/) and choose **Start with a demo** to hear
a sound right away. Files you import stay in this browser's library. If you
want to keep your work when you clear browser data or change computers,
[export a backup](../drawbar/backup.md#keeping-a-copy-in-the-browser).

Use the [browser comparison](support.md#the-library-in-the-browser) if you want
to connect a keyboard by USB, play a MIDI controller or open a folder on disk.

## On the desktop

Each download below is the latest release. Earlier versions are on the
[drawbar releases](https://github.com/jmoo/drawbar/releases?q=drawbar-v) page.

### Windows

<a class="button primary" href="https://drawbar.app/download/windows">Download for Windows</a>

Run the downloaded file. The app is unsigned, so Windows may ask before opening
it for the first time.

### macOS

<a class="button primary" href="https://drawbar.app/download/macos">Download for macOS</a>

For a Mac with Apple silicon running macOS 14 or later. Open the disk image,
drag **drawbar** into **Applications**, then open it there.

### Linux

<a class="button primary" href="https://drawbar.app/download/linux">Download for Linux</a>

For 64-bit Intel or AMD Linux with glibc 2.39 or later, such as Ubuntu 24.04 or
Debian 13. In the folder where you downloaded the AppImage, run:

```sh
chmod +x drawbar-*-x86_64-linux.AppImage
./drawbar-*-x86_64-linux.AppImage
```

If it reports that FUSE is missing, add `--appimage-extract-and-run` to the
second command. To connect a keyboard, also [allow USB access](#connecting-on-linux).

With Nix and flakes enabled, you can run the desktop app with
`nix run github:jmoo/drawbar#drawbar`. With Cargo, use `cargo install drawbar`
(Linux also needs the ALSA headers and `pkg-config`).
[Build from a checkout](../reference/building.md).

## The `nord` command

<a class="button" href="https://drawbar.app/download/nord-windows">nord for Windows</a>
<a class="button" href="https://drawbar.app/download/nord-linux-x86_64">nord for Linux x86-64</a>
<a class="button" href="https://drawbar.app/download/nord-linux-aarch64">nord for Linux ARM64</a>

On Windows, rename the download `nord.exe` and put it in a directory on your
`PATH`. On Linux, choose the archive for your processor, unpack it, and put
`nord` on your `PATH`. Earlier versions are on the
[nord-cli releases](https://github.com/jmoo/drawbar/releases?q=nord-cli-v) page.

With Rust installed, use `cargo install nord-cli`. With Nix and flakes enabled,
try `nix run github:jmoo/drawbar#nord-cli -- --help`. Once installed, follow
[Start with the nord command](../nord-cli/overview.md).

## Connecting on Linux

Linux needs a USB permission rule for drawbar and `nord` to reach an instrument.
Run these commands once to give access to the user signed in at the computer:

```sh
echo 'SUBSYSTEM=="usb", ATTR{idVendor}=="0ffc", TAG+="uaccess"' |
  sudo tee /etc/udev/rules.d/70-nord.rules
sudo udevadm control --reload
```

Unplug the instrument, plug it back in, then [connect in drawbar](../drawbar/instrument.md#connecting).

## If drawbar does not load

The loading page says which step failed and offers to try again.

- **A file could not be downloaded.** Reload the page. If an extension or a
  content blocker stopped it, allow drawbar.app and reload.
- **drawbar arrived but could not start.** drawbar needs WebAssembly and WebGL2,
  which every current browser has. If the page asks for hardware acceleration,
  turn it on in the browser's settings and restart the browser.
- **The browser cannot keep drawbar's library.** The page says so before
  downloading anything. Use a recent browser or the desktop app.
  [Supported browsers](support.md#the-library-in-the-browser).
