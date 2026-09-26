# Files

```sh
nord inspect patch.ne5p              # what is in a file
nord inspect *.ne5p                  # several at once
nord inspect --raw song.ne5t         # everything, as decoded
nord verify *.ne5p                   # check each re-encodes byte for byte
```

`inspect` prints a summary in the instrument's own terms: the parts and their
settings, the effects, and which piano and sample a program depends on. It exits
non-zero if any file fails to parse.

`verify` reads a file, writes it back, and checks that the bytes are identical,
reporting the offset of the first difference if not. Every supported format
passes, pianos and samples included. `nord piano verify --deep` also decodes every
recording inside, and `nord sample verify --deep` walks every encoded stroke.

## A file where a slot goes

`get`, `info` and `deps` take a file wherever they take a slot, so you can look
at a file without an instrument:

```sh
nord program get patch.ne5p            # the same summary as for a slot
nord program info patch.ne5p           # format, version, checksum
nord program deps patch.ne5p           # the piano and sample ids it refers to
```

A file stores ids, not names. `deps` on a slot asks the instrument for the
names. On a file, it prints the ids.

Changing what is inside a file is [Editing](editing.md).
