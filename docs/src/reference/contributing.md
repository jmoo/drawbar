# Contributing

[CONTRIBUTING.md](https://github.com/jmoo/drawbar/blob/master/CONTRIBUTING.md) is
the authority on style, tests, tooling and releases. Issues and pull requests are
on [GitHub](https://github.com/jmoo/drawbar/issues).

Two rules bear repeating. All reverse engineering is black-box work for
interoperability, from files and USB traffic produced by instruments the
researcher owns, with no decompiling of Clavia's software or firmware. And
nothing proprietary is committed: no factory presets, sound libraries, firmware,
or files shipped with Clavia software. Fixtures are made by this project's own
tools.

## This guide

The guide is an [mdBook](https://rust-lang.github.io/mdBook/) under `docs/`.
`nix build .#docs` renders it, and `nix develop -c mdbook serve docs` previews it
with live reload. Every push to master publishes it at
[drawbar.app/docs](https://drawbar.app/docs/), beside the app's latest release.
