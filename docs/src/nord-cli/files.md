# Working with files

```sh
nord inspect patch.ne5p              # readable summary
nord inspect --raw song.ne5t         # full Debug dump
nord verify *.ne5p                   # round-trip check
nord program get patch.ne5p          # the summary the slot form prints
nord program edit --fields           # what is settable
nord program edit patch.ne5p --set center_panel.gain=96 -o out.ne5p
```

`inspect` exits non-zero if any file fails to parse. `verify` parses each file,
writes it back, and checks the bytes are identical, reporting the offset of the
first difference if not. Every format round-trips, pianos and samples included.
`get`, `info` and `deps` take a file wherever they take a `BANK:SLOT`, so a file
on disk can be read with no instrument attached.

`edit` is the only verb that changes what is *inside* an object. A value is
spelled the way `nord inspect` and `--fields` print it, and one the field cannot
hold is rejected before anything is written. `--dry-run` writes nothing; editing
a file in place asks first, and `-o` writes somewhere else.

> ⚠️ **Some fields only mean something in pairs.** `center_panel.transpose` is
> ignored while `center_panel.transpose_enabled` is clear. Setting one half
> without the other warns; it is not refused.

`nord sample` also reaches the encoded audio; `--generation` selects v2, v3 or
v4, and **only v2 has been played on hardware**, so v3 and v4 must acknowledge
`--unverified`. `nord piano` rewrites piano libraries without re-encoding audio:
a trimmed library is hardware-verified for a dropped bank and for dropped
velocity layers, while renames, retunes, remaps and a narrowed key range are
inferred from specimens and have not been played.

Worked examples for every verb are in
[`crates/nord-cli/README.md`](https://github.com/jmoo/drawbar/blob/master/crates/nord-cli/README.md).
