# Trim or build a piano

A piano library (`.npno`) can hold several recordings of each note: soft and
loud strikes, the sound of the pedal, and the release when you lift your
finger. This page covers fitting a large piano into your keyboard's memory and
building a new one from WAVs.

## Trim to fit

When a piano is too large for your keyboard, choose which recordings are worth
the space for the part you'll play.

1. Connect the instrument so drawbar can read its free piano memory.
2. Under **This computer**, right-click the piano and choose **Duplicate**.
   Open the copy and find **Trim to fit** below the keys.
3. Compare the size kept with the space available, and cut until it fits. The
   meter suggests a cut that would free enough space.
4. Click keys across the range you'll play, especially at its ends. A hatched
   range is silent. You can switch a cut back on while you try others.
5. When the meter says the piano fits, choose **Save**. Use **Export…** if you
   also want the smaller file elsewhere.
6. [Send the piano to the keyboard](sending.md#sending).

Until the piano fits, it can't be queued, and **Queue send** reads as a
warning such as `Won't fit · 70 MB over`.

| If you play | Try cutting |
|---|---|
| Piano with the right hand only, in a split | **Keys**, narrowed to that range |
| The full keyboard | **Pedal resonance**, **Release samples** or a velocity layer |

Saving, exporting or queuing a piano can write the cuts into its file. To
discard trial cuts, choose **Revert** before any of those.

![Piano editor showing three recordings mapped across the keys, with Trim to fit below listing what is kept and its size](../assets/piano-key-map-light.png#gh-light-mode-only)
![Piano editor showing three recordings mapped across the keys, with Trim to fit below listing what is kept and its size](../assets/piano-key-map-dark.png#gh-dark-mode-only)

<details>
<summary>Cut single layers in Velocity layers</summary>

Each lane is a layer and each segment a recorded note, or *root*. Click a
segment to drop that layer for just that root.

The velocity ranges shown assume the program's **KBD Touch** is at 0. The
on-screen piano and MIDI preview play the loudest layer you've kept for each
key, so check the softer layers on the instrument.

</details>

## Change how the piano plays

| Section | What it changes |
|---|---|
| **Key map** | Drag the boundary between two roots to give keys to the neighbor. Drag the outer edges to change the range. |
| **Playback** | **Instrument gain** sets the overall level. **Keys ring on above** sets the highest key that is damped when you let go. **Kind** changes where the keyboard files the piano. |
| **Roots** | Open a row to hear a recording, save a WAV, adjust its level or drop that root. |
| **Per key** | Fine-tune one key, or draw a tuning change across several. |

<details>
<summary>Level steps in format 5.x and 6.x</summary>

Format 6.x adjusts a root's level in tenths of a decibel and can boost it.
Format 5.x trims in whole decibels.

</details>

## A new library

Use this route when you have **one WAV per stroke**. If you recorded several
notes into one long WAV, start with
[Make a piano from a recording](recording.md) instead. Takes build on their
own, not together with one-stroke WAVs.

1. Choose **New ▸ Piano library (.npno)…** and select your WAVs. Use 16-bit
   PCM, mono or stereo, at any sample rate.
2. Enter a **Name** and **Category**.
3. For each WAV, choose its root, its bank (attack, pedal resonance or
   release) and its layer. A filename such as `060-b0-l00.wav` fills in middle
   C, attack and the loudest layer.
4. For a sustained organ or pad, choose **Hybrid**, then open **Advanced** and
   select **Don't decay long notes**. For a piano, leave the box clear so the
   sound fades as you hold the key.
5. Choose **Create**. Try keys on the map and check the roots.
6. [Choose a destination and review the send](sending.md#sending).

<details>
<summary>Loops and releases</summary>

A loop marked in an attack WAV repeats while you hold the key. Audio after the
loop end is left out. With **Don't decay long notes**, the note keeps its
recorded decay and the loop holds steady.

A release WAV plays when you lift the key, at the level the note has reached
by then. Release WAVs cannot loop.

</details>

<details>
<summary>Format and template</summary>

A new library uses format 6.x. If your keyboard needs 5.x, choose it under
**Advanced ▸ Format**. To borrow playback settings from an existing piano,
choose it under **Advanced ▸ Template**. The template also sets the format.

</details>

[See which builds and edits have been played on hardware](../getting-started/support.md#piano-libraries).
