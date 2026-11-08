# npno-prep

**Cut a recorded take into the strokes of a Nord piano library.**

A take is one WAV holding every note played. npno-prep reads it, brings it to the
rate the instrument plays, and keeps the strokes as regions and loops in the take
itself, so any WAV editor can show them.

- `wav` reads PCM of 8 to 32 bits and IEEE float, plain or `WAVE_FORMAT_EXTENSIBLE`,
  and writes the same. Saving regions (`cue ` points named and spanned by a
  `LIST`/`adtl` chunk) and loops (`smpl`) replaces only those chunks: the audio
  and every other chunk are copied as they were.
- `resample` converts the rate with the filter `scipy.signal.resample_poly` uses by
  default, so its output matches the Python prototype's to float precision.
- `plan` finds the take's hits, assigns them to the notes played, chooses each
  note's velocity layers by level or playing order, and cuts each hit into an
  attack, from its strike to its key-up, and a release. Levels are measured
  against the take's floor with its mains hum taken out (`hum`). Run at the rate
  of the take it was tuned on, it places the same strokes as the Python
  prototype.
- `sustain` decides how each attack sustains while its key is held: a loop of whole
  periods tuned for the best match, or, for a note that dies before its key-up, a
  fade to silence. It also shapes them as the build reads them, evening out the
  decay across a loop and crossfading its end, so a mark moved in an editor is
  shaped the same way.
- `clean` takes a take's noise out of each stroke, as the build does: the hum
  under and around the note, and the hiss above it as the note dies.
- `filter` holds the Butterworth high-pass and notch sections these run on,
  forward only.

`nord piano prep TAKE.wav --notes ...` writes a take's hits into it as regions, and
`nord piano build TAKE.wav` builds a library from them.

## Disclaimer

Not affiliated with, authorized, or endorsed by Clavia DMI AB. "Nord" and "Clavia"
are trademarks of Clavia DMI AB, used here only to identify the hardware these
libraries are for.
