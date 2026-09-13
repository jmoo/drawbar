# Editing

`edit` changes fields inside a program, live slot, settings file, set list or
sample instrument, in a file or in a slot. Piano libraries have flags of their
own, covered in [Samples and pianos](libraries.md).

```sh
nord program edit --fields                                    # every field, and what it accepts
nord program edit patch.ne5p --set center_panel.gain=96 -o out.ne5p
nord program edit patch.ne5p --set effects_panel.fx1_rate=96 --yes    # in place
nord program edit 7:4 --set center_panel.split=true --dry-run         # on the instrument
nord program edit --set center_panel.gain=64 -o fresh.ne5p            # a new program
```

Editing a slot reads it, changes it and writes it back, so it asks first. Editing
a file in place asks too. `-o` writes somewhere else instead, and `--dry-run`
shows which fields and bytes would change without writing anything.

## Values

Spell a value the way `inspect` and `--fields` print it. A value the field cannot
hold is refused before anything is written:

```
$ nord program edit patch.ne5p --set center_panel.gain=200 --dry-run
error: "200" is not a value of gain (accepts 0 .. 127)
```

Some fields work in pairs. A transpose amount is ignored unless transpose is
enabled, so setting one without the other gets a warning.

## Live slots and settings

```sh
nord live edit 1:2 --set center_panel.gain=96 --yes
nord settings edit 1:1 --set fine_tune=0 --dry-run
```

Writing settings reloads the program on the panel, and unsaved panel changes are
lost. Store them first.

## Set lists and samples

A set list's fields are the slots it plays, `slot1` to `slot4`. A sample
instrument's are its name and each zone's root key and top note, plus its low
note in the layouts that store one. Notes are written as names (`C4` is middle
C) or as numbers, and `--fields` lists exactly what a given file offers.

```sh
nord setlist edit song.ne5t --set slot1=2:5 --set slot4=8:50 -o out.ne5t
nord sample edit inst.nsmp --set name="My Piano" --set zone2.top_note=C4 -o out.nsmp
```

## Any file

`nord edit` works out the format from the file itself, so it reaches everything
the commands above do, plus formats without a command of their own: Stage
programs and presets, and Sample Editor projects.

```sh
nord edit stage.ns3f --fields
nord edit stage.ns3f --set split_enabled=true -o out.ns3f
nord edit project.nsmpproj --set name=Marimba --set zone129.root_key=C3 --yes
```
