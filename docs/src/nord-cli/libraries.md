# Samples and pianos

Sample instruments (`.nsmp`) and piano libraries (`.npno`) hold encoded audio.
Both move to and from the instrument with `get` and `put`, like a program. The
verbs below reach the audio.

## Samples

```sh
nord sample decode inst.nsmp -o out/                 # every zone as a WAV
nord sample verify --deep inst.nsmp                  # walk every encoded stroke too
nord sample project new --zone a.wav=C3 --zone b.wav=C4 --name Marimba -o marimba.nsmpproj
nord sample build marimba.nsmpproj -o marimba.nsmp   # render a Sample Editor project
```

`encode` turns a single WAV into an instrument, and `build` renders a whole
project, loops and stereo included. `--generation 2`, `3` or `4` picks the
layout, and v2 is the default. v3 and v4 also need `--unverified`, because they
have not been played on an instrument.

Both take 16-bit PCM WAVs at 44.1 kHz, mono or stereo, in the plain or the
extensible WAV format. Convert anything else first.

## Pianos

```sh
nord piano inspect grand.npno                      # roots, layers, keys, tuning
nord piano inspect grand.npno --keys               # every key, its root and fine tune
nord piano decode grand.npno --key C4 --layer 0 -o c4.wav
nord piano edit grand.npno --name "My Grand" --tune C4=-2 --map C8=C7 -o out.npno
nord piano trim grand.npno --drop-bank release --layers 3 -o small.npno
nord piano split grand.npno --at C4 -o halves/
nord piano verify --deep grand.npno
```

`edit`, `trim` and `split` rewrite the library without touching its audio:
rename it, retune or reroute a key, drop a bank or the quieter layers, narrow the
range, or cut it in two. `trim` and `split` never write over their input, and
`edit` does so only with `--yes`.

## Building a piano library

`build` makes a library from a directory of WAVs, one per stroke, named
`<root>-b<bank>-l<layer>.wav`. `060-b0-l00.wav` is MIDI note 60, the attack bank,
the loudest layer. Banks are 0 for attack, 1 for pedal resonance and 2 for
release. The WAVs must be 16-bit PCM, mono or stereo, in the plain or the
extensible WAV format. Any sample rate is accepted.

```sh
nord piano build strokes/ --kind mallet --name Marimba -o marimba.npno
nord piano build strokes/ --template grand.npno --name Marimba -o marimba.npno
nord piano rebuild grand.npno -o again.npno        # re-encode a library's own strokes
```

Every key up to a semitone above the highest root plays the nearest root at or
above it. Keys beyond that are silent. Within a root, velocity picks the layer,
with the layers spread across the velocity range in order. To set a layer's
velocity value directly, name the file with `v12` in place of `l00`.

`build` writes piano format 6 unless `--format 5` asks for the older format, for
an instrument that does not play format 6. A template's format decides when you
give one. Layer values run from 0 to 135 in format 6 and from 0 to 27 in format 5;
`nord piano inspect` names a library's format.

A WAV with a loop in its `smpl` chunk, which most sample editors write, makes a
stroke that loops there while the key is held. Audio after the loop's end is not
used, and the loop's last four samples are replaced by the four just before its
start, which a seamless loop already has. While the key is held, the loop fades
the way the instrument's own pianos of that `--kind` do, faster on higher keys.
`--loop-fade` sets the rate at middle C in decibels per second, and `0` holds the
loop at its level. A stroke without a loop plays to its end, and a release stroke
cannot loop.
A loop is exact in a WAV at 35,002 Hz. At any other rate its ends move to the
nearest frame of the resampled audio.

Whatever the audio does not decide, such as decay and per-note tables, comes from
`--template` when you give one. Without a template, `--kind`, `--gain`,
`--damper-top` and `--loop-fade` set the playback values, and the rest uses neutral defaults. Each
stroke's level follows its layer either way.

[What is supported](../getting-started/support.md) says which of these libraries
have been played on an instrument.
