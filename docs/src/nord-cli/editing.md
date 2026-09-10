# Editing an object

`edit` is the only verb that changes what is *inside* an object, and it exists on
five nouns: `nord program edit`, `nord live edit` (the live buffer is the
program body under another tag, so the fields are identical), `nord settings
edit` (the menu settings, plus the `startup_*` state the instrument restores at
power-up), `nord setlist edit` (below), and `nord sample edit` (below). For the
first three the field paths are `nord-format`'s own names, generated from the
panel declarations, so `--fields` lists whatever the library currently knows:

```sh
nord program edit --fields                       # what is settable, and what it takes
nord program edit patch.ne5p --set center_panel.gain=96 -o out.ne5p
nord program edit patch.ne5p --set effects_panel.fx1_rate=96 --yes     # in place
nord program edit 7:4 --set center_panel.split=true --dry-run
nord program edit --set center_panel.gain=64 -o blank.ne5p             # a fresh program
```

A file and a slot are the same command; the slot form is a read-modify-write over
USB, so it asks before writing. Editing a file in place asks too — pass `-o` to
write somewhere else instead.

`live` and `settings` edit their slots as well. Live slots are `1:1` to `1:3`
and the settings singleton is `1:1`; neither class stores a name, so an edit
changes the body and nothing else:

```sh
nord live edit 1:2 --set center_panel.gain=96 --yes
nord settings edit 1:1 --set fine_tune=0 --dry-run
```

> ⚠️ **A settings write reloads the selected program on the instrument.** Panel
> state that has not been stored is lost, so re-`select` and re-apply afterwards.

## Values

**A value is spelled the way `nord inspect` and `--fields` print it**, and one
the field cannot hold is rejected before anything is written:

```
$ nord program edit patch.ne5p --set center_panel.gain=200 --dry-run
error: "200" is not a value of gain (accepts 0 .. 127)
```

A stored value the library does not recognize prints as `Unknown(9)`, so that is
also how it is written: `--set …=Unknown(9)`. A bare `9` matches nothing.

`--dry-run` reports the fields and the bytes that would change, and writes
nothing:

```
$ nord program edit patch.ne5p --set center_panel.transpose=-5 \
    --set center_panel.transpose_enabled=true --dry-run
center_panel.transpose_enabled           false -> true
center_panel.transpose                   0 -> -5
  byte 0x0018  0x01 -> 0xad  (body crc32)
  byte 0x0019  0xe8 -> 0x13  (body crc32)
  byte 0x001a  0x1d -> 0x5e  (body crc32)
  byte 0x001b  0xe7 -> 0x05  (body crc32)
  byte 0x0030  0x00 -> 0x01
  byte 0x0031  0x60 -> 0x10
```

> ⚠️ **Some fields only mean something in pairs.** `center_panel.transpose` is
> ignored while `center_panel.transpose_enabled` is clear, the instrument never
> clears that bit once it is set, and an untouched program holds `+1` rather than
> `0`. Setting one half without the other warns; it is not refused.

## `nord setlist edit`

A set list is the four program slots it points at, so those are its fields:
`slot1` to `slot4`, each taking a program address as the instrument shows it.

```sh
nord setlist edit song.ne5t --fields
nord setlist edit song.ne5t --set slot1=2:5 --set slot4=8:50 -o out.ne5t
nord setlist edit --set slot1=1:1 -o blank.ne5t     # a fresh set list
```

## `nord sample edit`

A sample instrument is mostly encoded audio, so its settable fields are the ones
the format can patch in place without touching a sample: the name, and each
zone's root key and top note. Notes are spelled as names (`C4`, `F#3` — middle C
is C4) or numbers 0–127, and zones are numbered from 1, top of the keyboard
first, the way `inspect` lists them:

```sh
nord sample edit inst.nsmp --fields
nord sample edit inst.nsmp --set name="My Piano" --set zone2.top_note=C4 -o out.nsmp
nord sample edit inst.nsmp --set zone1.root_key=48 --dry-run
```

The encoded audio itself is reached by the other `sample` verbs, under [samples
and pianos](libraries.md).

## `nord edit` — files with no noun

The top-level `edit` dispatches on the file itself rather than on an object
class, so it reaches every format `nord-format` can set: the Electro 5 bodies
above, the Stage 2/3/4 programs, the Stage 3/4 synth presets, the Stage 4
organ and piano presets — any body with a generated field registry — plus set
lists, sample instruments, and Nord Sample Editor projects (`.nsmpproj`: the
instrument name and velocity defaults, each zone's root key and key range,
each stroke's trim, loop, gain and velocity window, and each audio file's
path — zones and files under the ids `inspect` prints, a stroke under its
global id).

```sh
nord edit stage.ns3f --fields
nord edit stage.ns3f --set split_enabled=true -o out.ns3f
nord edit project.nsmpproj --set name=Marimba --set zone129.root_key=C3 --yes
nord edit project.nsmpproj --set stroke1.loop_enabled=on \
  --set stroke1.loop_start=1500 --set stroke1.gain=0.5 --yes
```
