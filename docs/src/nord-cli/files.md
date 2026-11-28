# Inspect files and unpack bundles

Run these commands in a folder with Nord files. Replace the example filenames
with yours, or use the [practice program](overview.md#try-it-without-a-keyboard).

## Identify and check a file

```sh
nord inspect practice.ne5p
nord inspect *.ne5p
nord verify practice.ne5p
```

For a program, `inspect` describes the parts, effects and the piano or sample
it uses. Use `--raw` for the full decoded values. If any input fails to parse,
`inspect` exits with a nonzero status.

`verify` checks a round trip in memory and reports the first differing byte
if the check fails. Use `nord piano verify --deep grand.npno` or
`nord sample verify --deep pad.nsmp` to check the encoded recordings too.
The general `verify` command can't round-trip ZIP bundles or backups.

## Find the piano or sample a program needs

```sh
nord program info practice.ne5p
nord program deps practice.ne5p
```

`info` shows the file format, version and checksum. `deps` prints the IDs
stored in the file. For names, connect the keyboard and give the program's
slot, for example `nord program deps 7:4`. Live slots report IDs even with
the instrument connected.

## Bundles

To take a program to another computer with the sounds it needs, read a bundle
from the keyboard. Substitute occupied slots on your own instrument. The
first example collects two programs. The second starts with a song and also
collects the programs it calls.

```sh
nord bundle get program 7:4 7:5 -o gig.ne5pbundle
nord bundle get setlist 1:2 -o set.ne5tbundle
```

`get` leaves the instrument unchanged. Read its warnings if a needed piano
or sample couldn't be matched to exactly one slot.

To work on a bundle you already have, choose a new folder name for `unpack`. An existing output folder is refused. Keep the
`meta.xml` file with the contents to preserve the manifest when repacking.

```sh
nord bundle unpack gig.ne5pbundle -o gig
nord bundle pack gig -o revised.ne5pbundle
```

Without a manifest, `pack` makes one listing which songs use which programs.
`inspect` and `unpack` read the uncompressed bundles Nord Sound Manager writes.
A compressed or damaged entry, repeated name or path outside the folder is
refused. [Supported bundle formats](../getting-started/support.md#files).
