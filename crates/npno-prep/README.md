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

`nord piano build TAKE.wav` builds a library from a take's labelled regions.

## Disclaimer

Not affiliated with, authorized, or endorsed by Clavia DMI AB. "Nord" and "Clavia"
are trademarks of Clavia DMI AB, used here only to identify the hardware these
libraries are for.
