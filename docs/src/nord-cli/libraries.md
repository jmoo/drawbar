# Work with samples and pianos

Choose the file type by where you want the result to play. A sample instrument
maps recordings to keyboard zones. A piano library can give each note several
layers for different playing strengths, plus separate resonance and release
sounds.

## Samples

For a one-zone instrument from a recording of middle C:

```sh
nord sample encode c4.wav --root-key C4 -o marimba.nsmp
nord sample verify --deep marimba.nsmp
```

For several zones, keep the mapping in a Sample Editor project so you can
adjust it and rebuild:

```sh
nord sample project new --zone c3.wav=C3 --zone c4.wav=C4 --name Marimba -o marimba.nsmpproj
nord sample build marimba.nsmpproj -o marimba.nsmp
```

Use 16-bit PCM WAVs at 44.1 kHz, mono or stereo, for `encode` and `build`.
The project finds relative WAV paths from its own folder. Both commands write
v2 by default. For v3 or v4, specify `--generation 3` or `--generation 4`
and acknowledge the playback status with `--unverified`. [Sample format support](../getting-started/support.md#files).

To extract the audio from an existing instrument, run
`nord sample decode marimba.nsmp -o zones/`. You get one WAV per decoded zone.
Read the summary for zones the decoder couldn't read.

## Pianos

Inspect a library first to find which roots, banks and layers it holds. A
*stroke* is one recording identified by those three values.

```sh
nord piano inspect grand.npno --keys
nord piano decode grand.npno --key C4 --bank attack --layer 0 -o c4.wav
```

To make a smaller library, drop release recordings or limit the number of
layers per root. Only request a bank the library contains. Inspect the result for keys left silent. To split a library
at middle C, use `split` with an output directory.

```sh
nord piano trim grand.npno --drop-bank release --layers 3 -o small.npno
nord piano inspect small.npno
nord piano split grand.npno --at C4 -o halves/
```

Use `nord piano edit` to rename, retune or remap. For example,
`nord piano edit grand.npno --name "My Grand" --tune C4=-2c -o tuned.npno`
lowers middle C by about two cents. For a remap such as `--map C8=C7`, choose
a destination root that the library actually contains. These edits, trimming
and splitting preserve the retained audio. Use `nord piano verify --deep` with
the resulting filename to check every stroke.

## Building a piano library

For individual recordings, put your WAVs in one directory. Use 16-bit PCM,
mono or stereo, at any sample rate. A name like `060-b0-l00.wav` describes
the recording:

| Part | Meaning |
|---|---|
| `060` | MIDI root note. 60 is middle C. |
| `b0` | Bank. Use 0 for the attack, 1 for pedal resonance and 2 for release. |
| `l00` | Loudest layer. Use `l01`, `l02` and so on for softer strikes. Use `v12`, for example, to give an explicit layer value. |

```sh
nord piano build strokes/ --kind mallet --name Marimba -o marimba.npno
nord piano inspect marimba.npno --keys
```

The new library uses format 6. Use `--format 5` for an instrument that needs
the older format. To borrow the playback settings and format of an existing
library, pass `--template grand.npno`. To re-encode a library's own strokes,
use `nord piano rebuild grand.npno -o rebuilt.npno`.

An attack WAV with a sample loop repeats while its key is held. The build uses
audio through the loop end. For pads and organs, use `--kind hybrid --no-decay`
to hold the loop steady and let releases ring out. For finer control,
`--loop-fade` sets the fade at middle C in decibels per second. Release strokes
cannot loop.

## Building from a recorded take

For one long recording, leave three to five seconds of quiet at the start,
keep peaks around −6 dBFS, and play notes individually. Strike a note at
different strengths to capture several layers. Use a separate output file to
keep the original recording. To try this with the guide's three-note example,
<a href="../assets/tine-take.wav" download>download the practice WAV</a>
and use `tine-take.wav` in place of `take.wav`.

```sh
nord piano prep take.wav -o regions.wav
nord piano build regions.wav --kind wurlitzer --name Wurli -o wurli.npno
nord piano verify --deep wurli.npno
```

`prep` writes a WAV with marks for attacks, loops, fades and releases. Read
its listing for misidentified pitches, skipped strikes and poor loop matches.
You can open that WAV in drawbar to [hear and adjust the cuts](../drawbar/recording.md#check-a-hit-by-ear)
before building. PCM takes from 8 to 32 bits and floating-point takes are
accepted. A take at another rate is resampled to 35,002 Hz.

| If the listing shows… | Try |
|---|---|
| A pitch assigned to the wrong note | Supply the notes you played in order, for example `--notes B1,D2,F#2,A2`. |
| Too many nearly identical layers | Use `--layers` to limit the count or increase `--min-layer-gap` to separate their levels. |
| A good quiet hit was skipped | Adjust `--min-snr`. The default requires a hit to stand 54 dB above the noise because boosting it also boosts hiss. |
| A stray hit or an overlong release | Use `--skip` with the hit number in the listing, or set `--release-seconds`. |

**One take per layer.** Record the same notes once per take and give the takes
loudest first. Each take keeps its layer wherever its recorded level falls.

```sh
nord piano prep loud.wav medium.wav soft.wav -o regions/
nord piano build regions/loud.wav regions/medium.wav regions/soft.wav --kind wurlitzer --name Wurli -o wurli.npno
```

**A folder of one-note recordings.** Run `nord piano prep "Upright Piano/" -o regions/`.
It searches subfolders, groups hits by pitch and level, and prints the build
command for the files it prepared. Check the listing for files it left out.

**Playback and cleaning.** A take saved with **Clean audio** off in drawbar
keeps its recorded noise. Otherwise the build reduces hum and hiss where it
can. With no template, `--kind` sets the playback defaults. If you omit it,
the build uses the category saved in the takes, or grand if none is set.
[Takes](../reference/takes.md) describes detection, cleaning and leveling in detail.
