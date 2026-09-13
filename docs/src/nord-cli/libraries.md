# Samples and pianos

Sample instruments (`.nsmp`) and piano libraries (`.npno`) hold encoded audio.
Both move to and from the instrument with `get` and `put`, like a program. The
verbs below reach the audio.

## Samples

```sh
nord sample decode inst.nsmp -o out/                 # every zone as a WAV
nord sample verify --deep inst.nsmp                  # decode every recording too
nord sample project new --zone a.wav=C3 --zone b.wav=C4 --name Marimba -o marimba.nsmpproj
nord sample build marimba.nsmpproj -o marimba.nsmp   # render a Sample Editor project
```

`encode` turns a single WAV into an instrument, and `build` renders a whole
project, loops and stereo included. `--generation 2`, `3` or `4` picks the
layout. Only v2 has been played on an instrument, so v3 and v4 require
`--unverified`.

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
range, or cut it in two. `trim` and `split` never write over their input. `edit`
does only with `--yes`.

## Building a piano library

`build` makes a library from a directory of WAVs, one per stroke, named
`<root>-b<bank>-l<layer>.wav`. `060-b0-l00.wav` is MIDI note 60, the attack bank,
the loudest layer. Banks are 0 for attack, 1 for pedal resonance and 2 for
release. Any sample rate is accepted.

```sh
nord piano build strokes/ --kind mallet --name Marimba -o marimba.npno
nord piano build strokes/ --template grand.npno --name Marimba -o marimba.npno
nord piano rebuild grand.npno -o again.npno        # re-encode a library's own strokes
```

Every key up to a semitone above the highest root plays the nearest root above
it. Keys beyond that are silent. Within a root, velocity picks the layer, with
the layers spread across the velocity range in order. Write `-v12` instead of
`-l00` to set a layer's velocity value directly.

Whatever the audio does not decide, such as decay and per-note tables, comes from
`--template` when you give one, and from neutral defaults when you do not. Both
kinds of library have been played on an instrument and sound the same.

## What has been played

Libraries drawbar has trimmed, built or rebuilt play on an instrument: mono and
stereo, every key including the lowest and highest root, all three attack
layers, the release stroke, and a vendor library re-encoded and indistinguishable
from the original. Renames, retunes, remaps and a narrowed key range have not been
played. See [What is supported](../getting-started/support.md).
