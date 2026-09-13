# Working with files

```sh
nord inspect patch.ne5p              # readable summary
nord inspect *.ne5p                  # several at once
nord inspect --raw song.ne5t         # full Debug dump
nord verify *.ne5p                   # round-trip check
```

`inspect` exits non-zero if any file fails to parse. For an Electro 5 program:

```
LA Grand.ne5p
  type:      Electro 5 program (ne5p)
  location:  bank 1 slot 5
  lower:     Piano  octave +0  sustain yes  control no
  upper:     Sample  octave +0  sustain yes  control no
  split:     no
  transpose: +1  (no)
  part mix:  49.6/50.0 (lower/upper %)
  gain:      119
  piano:     category 0  model 2  clav 0  acoustics 1  touch 2  mono no
  sample:    number 92  attack 14  decay/rel 87  dynamics 1  filter yes
  depends:   piano 0x3d4b3e14  sample 0x65d8c5a1
  fx:        stored value, with the panel's 0-10 reading where it applies
    fx1   off
    fx2   upper  chorus 1   rate 45  deep no
    fx3   off
    delay upper  feedback 2  tempo 24  wet 11 (0.9)  ping-pong no
    reverb stage      wet 23 (1.8)
    eq    lower        bass 74  freq 94  gain 70  treble 64
    rotary speed slow  stop off
```

`depends:` is the piano and sample the program references. Those ids are the same
values the instrument reports for that program over USB, so `nord program deps` on
the same slot will name them — which is the only way to resolve an id, since the
file itself stores no names.

For an organ program, both presets of all four models are shown, with the
selected model marked `*` and its active preset `<`:

```
  organ:     b3+bass selected (*), drawbar positions 0-8
   *b3    p1< 04.......  vib off  perc off
   *b3    p2  000000000  vib off  perc off
    vox   p1< 888800000  vib V3
    vox   p2  888800000  vib V3
   (* = selected model, < = its active preset)
```

Both presets are shown because in **b3+bass** the two are different instruments:
preset 1 is the bass manual, whose two drawbars live outside the nine-nibble
block. It renders as `04.......` rather than nine positions, since the nine
nibbles hold stale values in that mode.

Songs list their four program slots; settings print the decoded System, MIDI and
Sound menus plus the startup state the instrument restores at power-up; bundles
need the `bundle` feature (enabled here) to open.

## The slot verbs on a file

`get`, `info` and `deps` take a file wherever they take a `BANK:SLOT`, so a file
already on disk can be read with no instrument attached:

```sh
nord program get patch.ne5p             # the same summary the slot form prints
nord program get patch.ne5p --body -o patch.body   # strip the CBIN header
nord program info patch.ne5p            # the header: format tag, version, checksum
nord program deps patch.ne5p            # stored library ids
```

A path that exists wins over a slot reading, so a file named `7:4` is still a
file. What a file does not carry is reported as living on the instrument rather
than guessed at: files store no slot name, and `deps` on a file prints ids only —
the slot form asks the instrument, which attaches the names.

## `verify`

Parses each file, writes it back, and checks the bytes are identical — reporting
the offset of the first difference if not. It is the only thing that exercises
the write path end to end:

```
$ nord verify song.ne5t settings.ne5s grand.npno
ok     song.ne5t (62 bytes)
ok     settings.ne5s (78 bytes)
ok     grand.npno (209564996 bytes)
```

Every format round-trips, pianos and samples included. A piano library is
rebuilt from its parsed model — the per-root counts, every audio offset, the
alignment gap and the container checksum recomputed rather than carried — so the
byte-identical bar covers the whole container; `nord piano verify --deep` also
decodes every stroke it holds.

Changing what is *inside* a file is [editing an object](editing.md).
