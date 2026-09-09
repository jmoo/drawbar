# Install

The project is Nix-native. With [Nix](https://nixos.org/download/) installed and
flakes enabled, nothing else needs to be on the machine:

```sh
nix run github:jmoo/drawbar#nord-cli -- --help   # the CLI
nix run github:jmoo/drawbar#drawbar              # the desktop app
nix run github:jmoo/drawbar#drawbar-web          # drawbar in the browser
```

From a checkout of the repository, `.#` replaces `github:jmoo/drawbar#`. The
browser build is also hosted one level up from this guide:
[run drawbar in your browser](../).

## Browsers

drawbar reaches an instrument through WebUSB, which two browsers decline.

| Browser | Works |
|---|---|
| Chrome | yes |
| Edge | yes |
| Firefox | no — WebUSB declined |
| Safari | no — WebUSB declined |

> **Close Nord Sound Manager before connecting.** It claims the vendor interface
> exclusively, and nothing else — drawbar, `nord`, or Chrome — can attach
> alongside it.

Build details for each target are in
[`crates/drawbar/README.md`](https://github.com/jmoo/drawbar/blob/master/crates/drawbar/README.md).
