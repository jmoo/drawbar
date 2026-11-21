# Make a sample instrument

A sample instrument spreads a few recordings across the keyboard. Suppose you
recorded a marimba at C3 and C4. Each recording covers the keys near it, and
that range of keys is its *zone*. The zone's *root key* is the note you
recorded, where the sample plays at its original pitch.

## Start with WAV files

1. Prepare one WAV per recorded note, mono or stereo, in **16-bit PCM at
   44.1 kHz**. If your recorder uses 24-bit or 48 kHz, convert copies first.
2. Choose **New ▸ Sample Editor project (.nsmpproj)…** and select the WAVs.
   Name the project and set each root key: C3 and C4 for the marimba.
3. Choose **Create**. drawbar makes a folder holding the project and copies of
   its WAVs. Keep them together.
4. Open the project and shape the zones (see below).
5. Choose **Build → .nsmp** at the top. The instrument opens in its own tab.
   Click keys on either side of a zone boundary to hear where the sound
   changes.
6. [Send the instrument](sending.md#sending) when you're ready.

The build writes a v2 instrument beside the project and uses the project as it
is in drawbar, unsaved edits included. A later build gets a numbered name.
Keep the project for future changes. It also opens in Nord Sample Editor.

| To change | Do this in the project |
|---|---|
| Which keys a zone covers | Drag a zone edge on the **Key map**. |
| Root, trim, loop or crossfade | Open the zone's row in **Source strokes**. |
| The WAV a zone plays | Change its **Source file** in the same row. |
| How hard a key must be played | Set the zone's *velocity window*. |

<details>
<summary>Skip the project</summary>

If you already know the root keys and want only a playable instrument, choose
**New ▸ Sample instrument (.nsmp)…**, select your WAVs, set the roots and
choose **Create**. You won't have a project to rebuild from later.

</details>

## Open an existing project

Use **File ▸ Open folder…** on the folder that holds the project and its WAVs.
If the WAVs live beside the project folder, open their common parent. That
keeps the paths the project uses to find its WAVs. See
[Opening another folder](folders.md#opening-another-folder).

<details>
<summary>The build button says WAVs missing</summary>

Hover over it to see each filename and the reason. Check the **Source file**
path in each affected zone. It should lead from the project's folder to a WAV
inside the library. Two names that differ only in capitalization can be
ambiguous.

If the build stops on a setting the instrument can't reproduce, click the
status line for details.

</details>

## Edit a finished sample instrument

A `.nsmp`, `.nsmp3` or `.nsmp4` file carries its own audio, so you can change
it without the original WAVs. To keep the original, right-click it and choose
**Duplicate**, then open the copy.

- **Hear a zone.** Click a key or
  [play a MIDI controller](overview.md#midi-controllers). The line below the
  keys names the zone and its pitch shift, or says why a key is silent.
- **Move a boundary.** Drag the edge of a band above the keys. A hatched range
  has no zone.
- **Work on one recording.** Open its zone row for the waveform, root, top
  note, playback and **Save WAV…**.
- **Balance keys.** In a v2 instrument, the **Per key** section sets level and
  tuning. Drag across a lane, or use its table for one key.

Choose **Save** to keep the changes, or **Export…** for a copy to use
elsewhere. [Check sample format support](../getting-started/support.md#files).

<details>
<summary>Differences between v2, v3 and v4</summary>

In v2, moving one zone's top also moves the start of the next zone. In v3 and
v4 you can leave a gap between zones. The **Per key** section is for v2.

Velocity windows are read-only in a finished instrument; change them in the
source project. When a zone has a velocity window, how hard you strike a MIDI
key decides whether it plays.

</details>

## WAVs

To listen to a WAV, change its level or turn a recording of several notes into
a piano, see [Make a piano from a recording](recording.md).
