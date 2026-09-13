# drawbar

**Your Nord's sounds, in a window.** See what is on your computer and on your
instrument side by side, edit programs, samples and pianos, and send them back.
Runs in the browser or on the desktop.

![drawbar with an instrument attached and a piano library open in its editor](../../docs/src/assets/screenshot.png)

Open [drawbar.app](https://drawbar.app/) in Chrome or Edge, or run the desktop
app:

```sh
nix run github:jmoo/drawbar#drawbar
cargo install drawbar
```

- Drop Nord files in, or connect an instrument and read what it holds.
- Edit a program on a panel laid out like the instrument's, or any field in a
  table.
- Trim a piano library until it fits, build a sample instrument from WAVs, and
  listen to either before you send it.
- Queue your changes and send them in one go, with a review of what each one
  replaces.

Start with [The window](../../docs/src/drawbar/overview.md) in the user guide,
and read [What is supported](../../docs/src/getting-started/support.md) before
trusting alpha software with sounds you cannot re-create.

## Building

`nix develop`, then from `crates/`: `cargo run -p drawbar` and `cargo test -p
drawbar`. `nix build .#drawbar-web` makes the browser bundle. The rest is in
[Building from source](../../docs/src/reference/building.md).

## Disclaimer

Not affiliated with, authorized, or endorsed by Clavia DMI AB. "Nord", "Clavia",
and "Electro" are trademarks of Clavia DMI AB, used here only to identify the
hardware these formats come from.
