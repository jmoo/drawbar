# Make a sample instrument

Suppose you've recorded a marimba at C3 and C4. You can spread those two
recordings across the keyboard so that each covers the notes near it. The
range of keys that plays one recording is a *zone*. Its *root key* is the
note you recorded, where the instrument plays the sample at its original pitch.

## Start with WAV files

1. Prepare a WAV for each recorded note. For a sample instrument, use
   **16-bit PCM at 44.1 kHz**, mono or stereo. If your recorder uses 24-bit
   or 48 kHz, convert copies first.
2. Choose **New ▸ Sample Editor project (.nsmpproj)…** and select the WAVs.
   Give the project a name and set each root key. In the marimba example,
   assign the lower recording to C3 and the higher one to C4.
3. Choose **Create**. drawbar makes a folder containing the project and copies
   of its WAVs. Keep them together. The project remembers how to play those
   recordings and can also be opened in Nord Sample Editor.
4. Open the project. On the **Key map**, drag a zone edge to change which
   keys it covers. In **Source strokes**, open a row to adjust the root,
   trim the start or end, set a loop and crossfade, or change the source file.
   A *velocity window* sets how softly or firmly a key must be played to
   trigger that zone.
5. Choose **Build → .nsmp** at the top. The finished sample instrument opens
   in its own tab. Click keys on either side of a zone boundary to hear where
   the sound changes. [Send the built instrument](sending.md#sending) when
   you're ready.

The build uses the project and WAVs as they are in drawbar, including unsaved
edits. It writes a v2 instrument beside the project and gives a later build
a numbered name if needed. Keep the project for future changes.

If you already know the root keys and just want a playable instrument, choose
**New ▸ Sample instrument (.nsmp)…**, select your WAVs and choose **Create**
after setting the roots.

## Open an existing project

Use **File ▸ Open folder…** to open the folder that contains the project and
its source audio. Pick a common parent folder if the audio lives alongside the project folder.
This preserves the paths the project uses to find its WAVs.
[Opening a library folder](folders.md#opening-another-folder).

If the build button says **WAVs missing**, hover over it to see the filenames
and reasons. Check the **Source file** path in each affected zone. It should
lead from the project's folder to a WAV inside the library. Two names that
differ only in capitalization can be ambiguous. If the build stops on a
setting the instrument can't reproduce, click the status line for details.

## Edit a finished sample instrument

A `.nsmp`, `.nsmp3` or `.nsmp4` file carries its own audio. To keep the
original in your library, right-click it and choose **Duplicate** first. Open the copy
to change its key map or extract a recording without finding the original WAVs.

- **Hear a zone.** Click a key or [play a MIDI controller](overview.md#midi-controllers).
  The line below the keys names the zone, reports the pitch shift, or explains
  a silent key. The way you strike a MIDI key matters when a zone has a
  velocity window.
- **Move a boundary.** Drag the edge of a band above the keys. In v2, changing
  one zone's top also moves the start of its neighbor. In v3 and v4 you can
  separate zones. A hatched range shows keys with no zone.
- **Work on one recording.** Open its zone row for the waveform, root and top
  note, playback and **Save WAV…**. Velocity windows in a finished instrument
  are read-only. Change them in the source project.
- **Balance individual keys.** A v2 instrument has a **Per key** section for
  level and tuning. Drag across a lane or use its table for one key at a time.

Choose **Save** to keep the changes. **Export…** saves a file you can use elsewhere. [Check sample format support](../getting-started/support.md#files).

## WAVs

To listen to a WAV, adjust its level or turn a recording of several notes into
a piano library, see [Make a piano from a recording](recording.md).
