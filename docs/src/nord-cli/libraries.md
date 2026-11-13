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

`decode` names each zone it cannot read and why, and ends with a count of what
decoded. `encode` turns a single WAV into an instrument, and `build` renders a whole
project, loops and stereo included. A project's relative audio paths resolve from
the project's folder, with `/` or `\` between folders. `--generation 2`, `3` or
`4` picks the layout, and v2 is the default. v3 and v4 also need `--unverified`,
because they have not been played on an instrument.

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
nord piano build take.wav --kind wurlitzer --name Wurli -o wurli.npno
nord piano build strokes/ --template grand.npno --name Marimba -o marimba.npno
nord piano rebuild grand.npno -o again.npno        # re-encode a library's own strokes
```

Every key up to a semitone above the highest root plays the nearest root at or
above it. Keys beyond that are silent. Within a root, velocity picks the layer,
with the layers spread across the velocity range in order. To set a layer's
velocity value directly, name the file with `v12` in place of `l00`.

`build` writes piano format 6 unless `--format 5` asks for the older format, for
an instrument that does not play format 6. A template's format decides when you
give one. In format 6, `l00`, `l01`, … step 20 apart, spanning at least 0 to 50
and at most 0 to 100, and format 5 uses a fifth of each value. `nord piano inspect`
names a library's format.

A WAV with a loop in its `smpl` chunk, which most sample editors write, makes a
stroke that loops there while the key is held. Audio after the loop's end is not
used, and the loop's last four samples are replaced by the four just before its
start, which a seamless loop already has. While the key is held, the loop fades
the way the instrument's own pianos of that `--kind` do, faster on higher keys.
`--loop-fade` sets the rate at middle C in decibels per second, and `0` holds the
loop at its level. A stroke without a loop plays to its end, and a release stroke
cannot loop. A release plays at the level the note has fallen to at key-up, and
not at all when the attack recording is silent there.
A loop is exact in a WAV at 35,002 Hz. At any other rate its ends move to the
nearest frame of the resampled audio.

Whatever the audio does not decide, such as decay and per-note tables, comes from
`--template` when you give one, and each stroke's level follows its layer. Without a
template, `--kind`, `--gain`, `--damper-top` and `--loop-fade` set the playback
values. Without `--kind`, the kind the takes state, as drawbar's take editor saves
it, decides, and otherwise grand; takes that state different kinds need `--kind`.
Each stroke decays from note-on as the instrument's own pianos of that kind do,
faster on higher keys. For a sound that sustains, such as organ, strings or pads,
`--no-decay` holds every note and loop at its level, so a note has only its
recording's own decay, and lets each release ring out its recorded tail. The
instrument has no organ kind: build an organ, a pad or strings with
`--kind hybrid --no-decay`. In format 6, each layer is trimmed to play at the level of
its root's loudest layer, softer layers a little louder as that kind's are, up to 6
dB for a Wurlitzer, and each release 18 dB below the loudest. When a soft
layer or release needs more boost than the instrument gives at `--gain`, the whole
library is turned down to fit, but a release from a take just plays quieter. In
format 5, each stroke's level follows its layer.

## Building from a recorded take

`prep` finds the strokes in a recording of you playing, and `build` makes a library
of them:

```sh
nord piano prep take.wav -o regions.wav
nord piano build regions.wav --kind wurlitzer --name Wurli -o wurli.npno
```

`prep` names each hit's note by its pitch, and lists the notes it found. A note read
an octave off the notes played either side of it is put back in line. Each note may
be hit several times, loud to soft or in any order. Where the pitches mislead, give
the notes in the order you played them, as `--notes B1,D2,F#2,A2`: each hit then goes
to the nearest note in the list. Each note keeps up to `--layers` hits, spread evenly
in level, or the first ones played with `--order played`. `--min-layer-gap`, 1 dB
unless you change it, drops a
hit too close in level to a louder one, and `--skip` leaves out hits by their number
in the listing. A hit less than 54 dB over the take's noise is left out as too
quiet, since the build raises it and its hiss with it; `--min-snr` sets that limit.
`--max-seconds` and `--release-seconds` limit each attack and release.

The listing shows every hit: its note and pitch, its level, what became of it, and
how it sustains. A note still sounding at its key-up is looped, with a warning where
the loop matches poorly. A note that dies first fades to silence instead.

Or record one take per velocity layer, each playing every note once, and give them
loudest first:

```sh
nord piano prep loud.wav medium.wav soft.wav -o regions/
nord piano build regions/loud.wav regions/medium.wav regions/soft.wav --kind wurlitzer --name Wurli -o wurli.npno
```

Each take is one layer, whatever its level, so you may raise the gain for the softer
ones. A note played twice in a take keeps its last hit. `--skip 2:12` leaves out hit
12 of the second take.

Or give a folder of one-note files, such as a free sample set, its subfolders too:

```sh
nord piano prep "Upright Piano/" -o regions/
```

Each file's loudest hit is its note, named by its pitch across the whole set, so file
names are never read. A note's files become its layers by level, up to `--layers`,
and a file within `--min-layer-gap` dB of a louder one, such as a second round-robin,
is left out. Each file placed is written under `-o` at its own path, with the build
command for them all. A file with no hit, no pitch, or too little level over its
own noise is listed and left out.

`prep` never changes a take. A take at another rate is resampled to the instrument's
35,002 Hz in `-o`, with its other chunks. A take already at 35,002 Hz may be its own
`-o`, and only its regions and loops are replaced.

A take works best opening with 3 to 5 seconds of silence, free of ground loops,
peaking around -6 dBFS, and with each note held until it has died.

`build` makes a stroke of each region of a take, labeled as a stroke's WAV is named
(`060-b0-l00`), with a loop inside it as that stroke's loop. A region labeled
`084-b0-l00-fade` marks where that attack fades to silence. The take may be PCM of 8
to 32 bits or float. Each stroke is cleaned of the take's hum and hiss, its ends and
loop are shaped, and it is dithered to 16 bits. In a format 6 library without a
template, every region is also brought to one level, and the lowest and highest roots
are pitched out to reach all 88 keys. A take whose **Clean audio** drawbar's take
editor turned off is built as recorded. [Takes](../reference/takes.md) gives the
details.

[What is supported](../getting-started/support.md) says which of these libraries
have been played on an instrument.
