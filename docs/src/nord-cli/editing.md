# Edit a file or slot

To change a program's gain, start with the [practice program](overview.md#try-it-without-a-keyboard)
or substitute your own file. Use a fresh output name to keep the original.

```sh
nord edit practice.ne5p --fields
nord edit practice.ne5p --set center_panel.gain=96 --dry-run
nord edit practice.ne5p --set center_panel.gain=96 -o louder.ne5p
nord inspect louder.ne5p
```

`--fields` lists the names and values this particular file accepts.
`--dry-run` lists the fields and bytes that would change. The third command
writes the edited copy. Without `-o`, an edit to the input file asks for
confirmation.

## Pick a value

Use the spelling from `--fields`. A value outside the allowed range is
refused before writing. For example, the program gain accepts 0 through 127.
Repeat `--set` to change several fields together. If a setting needs a
switch, such as the amount and the enable switch for transpose, set both.

`nord edit` identifies the format from the file. Use it for a program, preset,
song, sample instrument or Sample Editor project. Piano libraries have
[editing and trimming options of their own](libraries.md#pianos).

## Songs, samples and projects

```sh
nord setlist edit song.ne5t --set slot1=2:5 --set slot4=8:50 -o revised.ne5t
nord edit pad.nsmp --set zone1.top_note=C4 -o lower-pad.nsmp
nord edit marimba.nsmpproj --fields
```

A song's four fields hold program slot addresses. For a sample, you can change
a zone's root and top notes, and its lower limit in formats that store it.
Note names use C4 for middle C. To rename a sample, check that its fields include
`name`. Instruments from the original Sample Library store no name.

## Edit an instrument slot

Use the command for the kind of slot. The first example previews a change to
a live slot. Remove `--dry-run` to review the write and apply it.

```sh
nord live edit 1:2 --set center_panel.gain=96 --dry-run
nord settings edit 1:1 --set fine_tune=0 --dry-run
```

A slot edit reads the current sound before changing it. Store any unsaved
panel changes before writing settings because the write reloads the selected
program. [Work with an instrument](instrument.md).
