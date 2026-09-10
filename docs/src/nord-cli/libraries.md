# Samples and pianos

The two library classes hold encoded audio: sample instruments (class 3, `.nsmp`)
and piano libraries (class 1, `.npno`). Their names and key maps are edited like
any other body — see [editing an object](editing.md) — and the verbs below reach
the audio.

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
narrowing the key range, and cutting a library in two. Nothing re-encodes audio — a
surviving stroke moves byte for byte — and `trim` and `split` refuse to write over
the file they read.

```sh
nord piano inspect grand.npno              # roots, layers per bank, keys, tuning
nord piano inspect grand.npno --strokes    # a line per stroke
nord piano decode grand.npno --key C4 --layer 0 -o c4.wav
nord piano edit grand.npno --name "My Grand" --tune C4=-2 --map C8=C7 -o out.npno
nord piano trim grand.npno --drop-bank release --layers 3 -o small.npno
nord piano split grand.npno --at C4 -o halves/
nord piano verify --deep grand.npno
```

A trimmed library loads on the instrument and plays at the original's level:
hardware-verified for a dropped bank and for dropped velocity layers. The other
edits — renames, retunes, remaps and a narrowed key range — are inferred from
specimens and have not been played.

A library is hundreds of megabytes, so moving one is `nord piano get` and `nord
piano put`, and the rest of the slot verbs address class 1 the way they address
programs.
