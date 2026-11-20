# Recorded takes

A take is a WAV containing one or more recorded strikes. Preparation identifies
each hit and marks its attack, loop or fade, and release. Building turns the
marked regions into strokes in a piano library.

The app's **Detect** works at the WAV's sample rate. The CLI's `nord piano prep`
resamples first, so their cuts can differ by a few frames on the same recording.
For the workflow, see [the WAV editor](../drawbar/recording.md) or
[the prep and build commands](../nord-cli/libraries.md#building-from-a-recorded-take).

## Finding the strokes

`prep` measures the take's floor with its mains hum taken out. It finds every hit
and names its note. Given a note list, a hit goes to the next note in the list whose
pitch is nearest. With none, a hit is named by its own pitch, and a note read an
octave off the notes played either side of it is put back in line. By level, a
note's hits within `--min-layer-gap` dB of a louder one are dropped, and the rest are
spread evenly in dB. With `--order played`, the first hits played are kept, the
first the loudest.

For a continuous take, the noise floor is the tenth percentile of the levels of
its 10 ms frames. A single-note file often contains too little silence for that
measure. For each file in a directory of one-note recordings, `prep` instead looks for a
half-second rest after the loudest frame. It selects the qualifying interval
with the lowest median level. A candidate must meet all of these conditions:

- Its halves differ by at most 1 dB.
- It lies at least 10 dB below the loudest frame.
- Most of the interval contains a signal above digital silence.
- No later half-window is more than 6 dB quieter.
- The interval passes a pitch check. It is rejected as a continuing note if
  most of the first four harmonics below the Nyquist frequency peak more than
  6 dB above their neighboring bands.

Only the loudest hit counts. If the file starts at the strike, it can start with
a hit 15 dB above the rest. Without a measurable rest, the file skips the
signal-to-noise cutoff and builds without noise cleaning. A file of steady
noise alone can therefore be mistaken for a held note. Files written by
`prep` record which floor measurement the build should use.

A folder's hits are sorted by pitch before their notes are named, so a stretched
note has its neighbors to go by. Each keeps the octave its pitch was read in.

A hit standing less than the
[`--min-snr`](../nord-cli/libraries.md#building-from-a-recorded-take) limit over the
floor is left out: leveled with the others, its hiss would come up with it. Each kept
hit becomes an attack, from its strike to its key-up, and a release from the key-up.
A key-off that stands clear of neither the quiet before its hit nor the floor gets no
release.

## Sustain

Each attack's sustain is found on the attack as the build cleans it. A note still
sounding at its key-up is looped. The loop is about half a second, a whole number of
the note's periods, ending 150 ms before the key-up or where the note sinks to 30 dB
over the floor. It is tuned within a period for the best match between its end and
its start, and the listing warns where the match is under 0.5.

A note that dies into the floor before its key-up, and would finish fading before
it, gets a fade region instead. The fade starts where the note came within 20 dB of
the floor, and the attack ends where the fade reaches silence. This keeps background noise at the tail of a note out of the loop.

## Regions

An attack is labeled `060-b0-v00`, a release `060-b2-v00`, and a fade
`060-b0-v00-fade`, where `v00` is the layer value. A region labeled any other way,
or a loop that crosses a region's end, is refused. So is a fade that does not end
with its attack, or that names a release or a looped attack.

A take at another rate is resampled to 35,002 Hz at its own bit depth, and the
regions are placed on the resampled audio. Its other chunks are kept as they were,
and a `smpl` chunk keeps its unity note and sampler data.

## Cleaning

The take's mains hum is found and followed as its frequency wanders, and its hiss is
measured in its quietest stretch. Each stroke then loses everything under 0.7 times
its root's fundamental, which takes the hum and the action's knock with it. A note
below F4 also has its strongest hum lines notched where they lie clear of the note's
own partials. As the note dies, a low-pass follows its top end down, so the hiss
above it goes too, while the strike keeps all its treble. Nothing is touched where
the note stands over the hiss.

## Shaping

Each attack fades in over 0.3 ms, and each release over 5 ms, fading out over its
last 40 ms. A loop has the decay across it evened out, so its end meets its start,
and its last 30 ms are crossfaded into the audio before its start. A fade follows
the note's own decay as it slows, down to 70 dB under its peak, and the attack then
loops on 100 ms of silence.

Each region is dithered to 16 bits: rounded after adding a little noise, so the
quietest part of a decay turns to a steady hiss rather than distortion. Silence
stays silent.

## Levels

A format 6 library built without a template plays each stroke at the level of its
root's loudest layer, whatever the stroke's own level, and each root at the level of
its loudest layer's audio. So before rounding, `build` brings every region of a take
to a peak of -13 dBFS. The soft layers keep their detail above the 16-bit steps, and
a note recorded quieter plays as loud as the others. The library grows, because
louder audio takes more bits to code.

Such a library also reaches all 88 keys where it can. The lowest root's strokes are
pitched down, and the highest's up, by at most 12 semitones, to roots that reach A0
and C8.

A release from a take plays at its recorded level under its own attack, the region
of its root at its layer, both measured once cleaned. It is lifted by at most 12 dB
toward 14 dB under the attack, so a quiet key-off stays quiet without being lost. A
release with no such attack, or one from a directory of WAVs, plays 18 dB under its
root's loudest attack.

A format 5 library, or one built against a template, plays each stroke's audio at
its own level, so nothing is raised.
