# Takes

How `nord piano prep` reads a recorded take, and how `nord piano build` turns a
take's regions into strokes. [Samples and pianos](../nord-cli/libraries.md#building-from-a-recorded-take)
says how to use them.

## Finding the strokes

`prep` measures the take's floor with its mains hum taken out. It finds every hit
and gives it to the next note in the list whose pitch is nearest. By level, a
note's hits within `--min-layer-gap` dB of a louder one are dropped, and the rest are
spread evenly in dB. With `--order played`, the first hits played are kept, the
first the loudest.

A take's floor is the level a tenth of its 10 ms frames are under. A one-note file
of a folder holds its note for most of its length, so its floor is the mean level of
its quietest half second instead, and only its loudest hit counts. A folder's hits
are sorted by pitch before their notes are named, so each has its neighbors to go by.

A hit standing less than 54 dB over the floor is left out: leveled with the others,
its hiss would come up with it. Each kept hit becomes an attack, from its strike to
its key-up, and a release from the key-up. A key-off that stands clear of neither
the quiet before its hit nor the floor gets no release.

## Sustain

Each attack's sustain is found on the attack as the build cleans it. A note still
sounding at its key-up is looped. The loop is about half a second, a whole number of
the note's periods, ending 150 ms before the key-up or where the note sinks to 30 dB
over the floor. It is tuned within a period for the best match between its end and
its start, and the listing warns where the match is under 0.5.

A note that dies into the floor before its key-up, and would finish fading before
it, gets a fade region instead. The fade starts where the note came within 20 dB of
the floor, and the attack ends where the fade reaches silence. A loop in the noise of
a dying note clicked, so a dying note is never looped.

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
