# Contributing

[CONTRIBUTING.md](https://github.com/jmoo/drawbar/blob/master/CONTRIBUTING.md) is
the authority on the research boundary, style, tests, tooling and releases. Issues
and pull requests are on [GitHub](https://github.com/jmoo/drawbar/issues).

Two rules bear repeating. All reverse engineering is black-box work for
interoperability, from files and USB traffic produced by instruments the
researcher owns, with no decompiling of Clavia's software or firmware. And
nothing proprietary is committed: no factory presets, sound libraries, firmware,
or files shipped with Clavia software. Public fixtures come from the project's tools or from the instrument panel.

## This guide

The guide is an [mdBook](https://rust-lang.github.io/mdBook/) under `docs/`.
`nix build .#docs` renders it, and `nix develop -c mdbook serve docs` previews it
with live reload. It is published at [drawbar.app/docs](https://drawbar.app/docs/).
