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

`build` also takes one WAV holding every stroke, such as a recorded take, whose
regions are the strokes. Each region is labelled as a stroke's WAV is named
(`060-b0-l00`), and a loop inside a region is that stroke's loop. The take may be
PCM of 8 to 32 bits or float, and each region is dithered to 16 bits: rounded
after adding a little noise, so the quietest part of a decay turns to a steady
hiss rather than distortion. Silence stays silent. A region labelled any other way, or a loop that crosses a
region's end, is refused.

A take's strokes are cleaned first. The take's mains hum is found and followed
as its frequency wanders, and its hiss is measured in its quietest stretch. Each
stroke then loses everything under 0.7 times its root's fundamental, hum and the
action's knock with it. A note below F4 also has its strongest hum lines notched
where they lie clear of the note's own partials. As the note dies, a low-pass
follows its top end down, so the hiss above it goes with it, while the strike
keeps all its treble. Nothing is touched where the note stands over the hiss.
A take whose **Clean audio** drawbar's take editor turned off is built as
recorded, with none of this.

A take's strokes are shaped as they are built. Each attack fades in over 0.3 ms,
and each release over 5 ms, fading out over its last 40 ms. A looped region's loop
has the decay across it evened out, so its end meets its start, and its last 30 ms
are crossfaded into the audio before its start. A region labelled with `-fade`
after a stroke's name (`084-b0-l00-fade`) lies at the end of that attack, and the
attack fades from there to silence at its end, following the note's own decay as
it slows, down to 70 dB under its peak, then loops on 100 ms of silence. A fade
that does not end with its attack, or that names a release or a looped attack, is
refused.

A format 6 library built without a template plays each stroke at the level of
its root's loudest layer, whatever the stroke's own level, and each root at the
level of its loudest layer's audio. So before rounding, `build` brings every
region of a take to a peak of -13 dBFS. The soft layers keep their detail above
the 16-bit steps, and a note recorded quieter plays as loud as the others. The
library grows, because louder audio takes more bits to code. Such a library also
reaches the whole 88 keys where it can: the lowest root's strokes are pitched
down, and the highest's up, by at most 12 semitones, to roots that reach A0 and
C8. A release from a take plays at its
recorded level under its own attack, both measured once cleaned, the region of its root at its layer, lifted by
at most 12 dB towards 14 dB under it, so a quiet key-off stays quiet without being
lost. A release with no such attack, or one from a directory of WAVs, plays 18 dB
under its root's loudest attack. A format 5 library, or one built
against a template, plays each stroke's audio at its own level, so nothing is
raised.

`prep` writes those regions. Give it the notes in the order you played them, and
the number of velocity layers each note gets:

```sh
nord piano prep take.wav --notes B1,D2,F#2,A2 --layers 4 -o regions.wav
```

Each note may be hit several times in a row, loud to soft or in any order. `prep`
finds every hit, gives it to the next note in the list whose pitch is nearest, and
keeps up to `--layers` of each note's hits: by measured level, dropping hits within
`--min-layer-gap` dB of a louder one and spreading the rest evenly in dB, or with
`--order played` the first ones played, the first the loudest. Each kept hit
becomes an attack, from its strike to its key-up and at most `--max-seconds`
long, and a release from the key-up, at most `--release-seconds` long. A hit
standing less than 54 dB over the take's floor, measured with its mains hum taken
out, is left out: levelled with the others, its hiss would come up with it. A
key-off that stands clear of neither the quiet before its hit nor the floor gets
no release. `--skip` leaves out hits by the number the listing gives them.

Each attack also gets its sustain, found on the attack as the build cleans it. A
note still sounding at its key-up is looped:
about half a second, a whole number of its periods, ending 150 ms before the key-up
or where it sinks to 30 dB over the floor, tuned within a period for the best
match between its end and its start. The listing gives each loop's match, and warns
under 0.5. A note that dies into the floor before its key-up, and would finish
fading before it, gets a fade region instead, from where it came within 20 dB of the
floor, and its attack ends where the fade reaches silence. Loops in the noise of a
dying note clicked, so it is never looped.

Or record one take per velocity layer, each playing every note once, and give them
loudest first:

```sh
nord piano prep loud.wav medium.wav soft.wav --notes B1,D2,F#2,A2 -o regions/
nord piano build regions/loud.wav regions/medium.wav regions/soft.wav --kind wurlitzer --name Wurli -o wurli.npno
```

Each take is one layer, whatever its level, so you may raise the gain for the softer
ones. A note played twice in a take keeps its last hit. Each take's floor and hum
are measured on their own, and each is written into `-o` under its own name.
`--skip 2:12` leaves out hit 12 of the second take.

A take at any other rate is resampled to the instrument's 35,002 Hz first, at its
own bit depth, and the regions are placed on that, so they lie where the build
reads them, and the original is never changed. The take's other chunks go with it as
they were; a `smpl` chunk keeps its unity note and sampler data. A take already at 35,002 Hz may be
its own `-o`: only its regions and loops are replaced. The listing shows every hit: its note
and detected pitch, its level, what became of it, and how it sustains.

A take works best opening with 3 to 5 seconds of silence, free of ground loops,
peaking around -6 dBFS, and with each note held until it has died.

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
values; without `--kind`, a take's own kind, as drawbar's take editor saves it,
decides, and grand otherwise (takes that state different kinds are refused), and each stroke decays from note-on as the instrument's own pianos of that
kind do, faster on higher keys. In format 6, each layer is trimmed to play at the
level of its root's loudest layer, softer layers a little louder as that kind's are,
up to 6 dB for a Wurlitzer, and each release 18 dB below the loudest. When a soft
layer or release needs more boost than the instrument gives at `--gain`, the whole
library is turned down to fit; a release from a take just plays quieter. In format 5, each stroke's level follows its layer.

[What is supported](../getting-started/support.md) says which of these libraries
have been played on an instrument.
