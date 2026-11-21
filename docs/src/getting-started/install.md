# Open or install drawbar

## In the browser

[Open drawbar](https://drawbar.app/) and choose **Start with a demo** to hear
a sound right away. Files you import stay in this browser's library. If you
want to keep your work when you clear browser data or change computers,
[export a backup](../drawbar/backup.md#keeping-a-copy-in-the-browser).

Use the [browser comparison](support.md#the-library-in-the-browser) if you want
to connect a keyboard by USB, play a MIDI controller or open a folder on disk.

## On the desktop

Find the latest release whose name begins with **drawbar-v** and open its
**Assets** list. Choose the download for your computer. [drawbar releases](https://github.com/jmoo/drawbar/releases?q=drawbar-v).

### Windows

Download the file ending in `-x86_64-windows.exe` and run it. The app is
unsigned, so Windows may ask before opening it for the first time.

### macOS

On a Mac with Apple silicon running macOS 14 or later, download the file ending
in `-aarch64-macos.dmg`. Open the disk image, drag **drawbar** into
**Applications**, then open it there.

### Linux

On 64-bit Intel or AMD Linux with glibc 2.39 or later (such as Ubuntu 24.04 or
Debian 13), download the file ending in `-x86_64-linux.AppImage`. In the folder
where you downloaded it, run:

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

For a terminal, use the latest **nord-cli-v** release. On Linux, download the
`nord` archive ending in `-x86_64-linux.tar.gz` or `-aarch64-linux.tar.gz`
for your processor. Unpack it and put `nord` in a directory on your `PATH`.
On Windows, download the file ending in `-x86_64-windows.exe`, rename it
`nord.exe`, and put it on your `PATH`. [nord-cli releases](https://github.com/jmoo/drawbar/releases?q=nord-cli-v).

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
