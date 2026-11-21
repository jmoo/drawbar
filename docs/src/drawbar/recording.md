# Make a piano from a recording

Record yourself playing single notes into one WAV, and drawbar turns it into a
piano library. It finds the notes, lets you check where each starts, loops and
ends, and builds a file the keyboard plays. If each recording is already its
own WAV, [build from individual files](pianos.md#a-new-library) instead.

The whole recording is the *take*. Each strike in it is a *hit*.

## Try the practice take

<a class="button" href="../assets/tine-take.wav" download>Download the practice WAV</a>

It holds three notes from drawbar's demo electric piano, with a little quiet
before the first strike.

1. Drop the file on drawbar and double-click **tine-take** under
   **This computer**.
2. Choose **Detect**. Look for **E3**, **E5** and **E7** over the waveform.
3. Select **E3** and try **Loop wrap** to hear where the held note repeats.
4. Select **E5**. That note dies away quickly enough to get a fade.

You can [build a piano](#build-and-try-the-piano) from these marks or adjust
them first.

## Record and find the notes

1. Start recording with three to five seconds of quiet, so drawbar can hear
   the background noise.
2. Leave headroom. A peak around −6 dB on your recorder is a good start.
3. Play one note at a time and let it die away. For changes in touch, strike
   the same note softly and firmly, with space between strikes.
4. Drop the WAV on drawbar and double-click it under **This computer**. The
   top waveform shows the whole take. **Play** plays it.
5. Choose **Detect**. A box appears around each hit, labeled with its note and
   layer, such as **C4 v22**. The message above the waveform says how many
   hits were marked or skipped.

**Detect** replaces any marks already in the take. Each hit of a note becomes
one of its layers, ordered by level.

<details>
<summary>Quiet notes are missing</summary>

Lower **Skip hits … or less over noise** and choose **Detect** again. Listen
for hiss: bringing a quiet recording up can bring its noise up too. For one
missed note, add it by hand (see below).

</details>

## Check a hit by ear

![The practice take with E3 selected. The upper waveform shows all three strikes. In the close-up below, two markers enclose the loop inside the attack, and the release follows on the right.](../assets/take-editor-light.png#gh-light-mode-only)
![The practice take with E3 selected. The upper waveform shows all three strikes. In the close-up below, two markers enclose the loop inside the attack, and the release follows on the right.](../assets/take-editor-dark.png#gh-dark-mode-only)

Click a box in the top waveform to see that hit below. The **attack** starts
when you strike the note, the **loop** repeats while you hold the key, and the
**release** sounds when you lift it.

| To hear… | Use |
|---|---|
| The passage as recorded | **Play original** |
| The note held and released | Hold **Play hit**, then let go. A quick click plays it once. |
| Whether a loop clicks where it repeats | **Loop wrap**. Click again to stop. |
| Only the sound of lifting the key | **Release** |

The fades you hear are close to the instrument's, not exact.

<details>
<summary>Moving around and seeing quiet sounds</summary>

The left and right arrows below the hit step to its neighbors. If the release
is hard to see, **+** beside the waveform makes quiet sounds taller and **−**
takes it back. These buttons change only the view.

</details>

## Correct a note or loop

- **A note starts late or runs into the next sound.** Drag its start or end
  marker in the lower waveform. **Zoom in** gives a closer view; drag the
  waveform sideways to move along it. **Whole hit** shows both ends again.
- **A loop clicks.** Choose **Loop** to zoom to the join, then move its
  markers while listening with **Loop wrap**. Markers snap to nearby points
  where the waveform crosses zero. Hold Alt to place one exactly.
- **A note was missed.** Click the strike in the upper waveform and choose
  **Add ▸ Hit**.
- **A note is mislabeled.** Type a note and layer such as `D4 v30` above the
  lower waveform, then leave the box. Escape cancels. A red name is invalid or
  already taken.
- **A note needs a loop or release.** Select the hit, choose **Add ▸ Loop** or
  **Add ▸ Release**, and adjust the markers.

**Delete** removes a hit, loop or release mark. The recording stays intact.

## Build and try the piano

1. Choose the take's **Category**. It sets the new piano's playback defaults.
2. Leave **Clean audio** on to reduce hum and hiss, or turn it off to keep the
   recorded sound. A long take can take a few seconds to analyze.
3. Choose **Build piano library…** at the top. It uses the current marks,
   unsaved edits included. Check the name and category, then choose
   **Create**.
4. The piano opens in a new tab. Play a few notes on its key map, and listen
   to the change between neighboring notes. To fix a cut or loop, adjust the
   mark in the WAV tab and build again.
5. In the WAV tab, choose **Save** to keep the marks and the cleaning choice
   for next time.
6. [Trim or send the piano](pianos.md#trim-to-fit).

## Adjust a WAV's level

The WAV editor also works on its own.

1. Set **Gain** and check the clipping message.
2. Choose **Apply gain**.
3. Choose **Save**. The audio is written at its new level, in the WAV's
   original sample format.

Gain is relative to the last save: applying 10 dB and then 12 dB raises the
level by 12 dB. Set 0 dB to return to the saved level.

<details>
<summary>WAVs the editor opens</summary>

The editor reads uncompressed 8-, 16-, 24- and 32-bit PCM and 32- or 64-bit
floating-point WAVs, at 8 kHz or more. In the browser, a WAV can be at most
120 MB; for larger ones use the
[desktop app](../getting-started/install.md#on-the-desktop). A marked take can
be built from any of these. The
[one-WAV-per-stroke piano builder](pianos.md#a-new-library) and the
[sample instrument builder](samples.md#start-with-wav-files) have their own
requirements.

</details>

For how drawbar reads a take, see [Takes](../reference/takes.md).
