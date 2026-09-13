# Samples and pianos

The two library classes hold encoded audio: sample instruments (class 3, `.nsmp`)
and piano libraries (class 1, `.npno`). A sample instrument's name and zones are
edited like any other body — see [editing an object](editing.md) — and a piano
library's name, tuning and key map with `nord piano edit`, below. The other verbs
below reach the audio.

## Sample audio

`nord sample` also reaches the encoded audio. `decode` writes each zone as a
WAV at the format's own field rate, `verify --deep` walks every stroke's stream
against the codec's grammar, `encode` turns a WAV into a sample instrument,
`build` renders a whole Sample Editor project (zones, loops, stereo), and
`project new` writes a project from WAVs for the editor to open.

A build is the editor's own output byte for byte apart from a float residue in
the resampling kernel: the odd audio field lands one count out, and nothing the
instrument plays changes. `--plain` opts out of the record coding the editor
picks and states every content field outright, which decodes back the same audio
from a larger file. `--generation` selects v2, v3 or v4. Only v2
has been played on hardware — mono, stereo and looped — so v3 and v4 must
acknowledge `--unverified`.

```sh
nord sample decode inst.nsmp -o out/
nord sample verify --deep inst.nsmp
nord sample project new --zone a.wav=C3 --zone b.wav=C4 --name Marimba -o marimba.nsmpproj
nord sample build marimba.nsmpproj -o marimba.nsmp
nord sample build marimba.nsmpproj --generation 4 -o marimba.nsmp4 --unverified
```

## `nord piano`

A piano library is a directory of strokes — one recording per root note, bank and
velocity layer — and the encoded audio those strokes own. `inspect` reports that
directory, `decode` writes one stroke to a WAV at the rate the instrument plays it,
and `edit`, `trim` and `split` rewrite the container: renaming, retuning a key,
rerouting a key to another root, dropping a bank or the quieter velocity layers,
narrowing the key range, and cutting a library in two. None of those re-encodes
audio — a surviving stroke moves byte for byte. `trim` and `split` refuse to write
over the file they read; `edit` writes over it only with `--yes`, and `-o` writes
somewhere else instead.

```sh
nord piano inspect grand.npno              # roots, layers per bank, keys, tuning
nord piano inspect grand.npno --strokes    # a line per stroke
nord piano inspect grand.npno --keys       # every covered key, its root and fine tune
nord piano decode grand.npno --key C4 --layer 0 -o c4.wav
nord piano edit grand.npno --name "My Grand" --tune C4=-2 --map C8=C7 -o out.npno
nord piano trim grand.npno --drop-bank release --layers 3 -o small.npno
nord piano split grand.npno --at C4 -o halves/
nord piano verify --deep grand.npno
```

## Building a piano library

`build` and `rebuild` do write audio, and neither writes over the library it reads.
`build` lays a whole library out from a directory of WAVs named
`<root>-b<bank>-l<layer>.wav` — `060-b0-l00.wav` is MIDI note 60, the attack bank,
the loudest layer — resampling any rate onto the lattice the instrument plays at.
Banks are 0 attack, 1 pedal resonance and 2 release. `rebuild` codes a library's own
strokes again from the frames they decode to and prints how each one's blocks came
back: a file this coder wrote comes back byte for byte, one it did not comes back
block for block.

```sh
nord piano build strokes/ --template grand.npno --name Marimba -o marimba.npno
nord piano build strokes/ --kind mallet --name Marimba -o marimba.npno
nord piano rebuild grand.npno -o again.npno
```

Two rules decide what a built library plays. Every key up to one semitone above the
highest root sounds, playing the nearest root at or above it, and keys past that are
left uncovered — so a library of roots C2, C3 and C4 covers everything up to C#4 and
nothing above. Within a root, a key sounds the largest layer value the root holds
that is at most `(127 − velocity)·31/127`; `l00`, `l01`, … are spread over 0..27 so
each layer answers to its own part of the velocity range, and `v12` in place of `l00`
in a WAV's name states a layer's value outright. One root's bank names all its layers
the same way.

Everything the audio does not decide comes from `--template`: the length marks, the
decay coefficients, the per-note tables, the playback parameters, the stream version
and the word at the body's start. Each new stroke inherits from the template stroke
of its own bank and nearest root. The instrument accepts those fields as the
template donated them, and what it makes of them beyond accepting is not known.
Given no template, `build` states them by rule, and they are then neutral playback
parameters: no decay applied over the recordings, the layer trims taken from the
layer values, and the damper limit `--kind` implies. A library written that way has
been played and sounds like the same audio built against a template.

## What has been played

A library written here loads on the instrument and plays: hardware-verified for a
trim, both for a dropped bank and for dropped velocity layers, and for what `build`
and `rebuild` code — mono and stereo, every key of a full-keyboard library including
its lowest and highest root, each of three attack layers, the release stroke at
note-off, a long stroke to its end, the keys between roots transposed, and a vendor
library coded again playing indistinguishably from the original in level and in
spectrum. The other edits — renames, retunes, remaps and a narrowed key range — are
inferred from specimens and have not been played.

## Moving a library

A library is tens of megabytes, so moving one is `nord piano get` and `nord
piano put`, and the rest of the slot verbs address class 1 the way they address
programs.
