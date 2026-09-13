# Samples

drawbar edits sample instruments (`.nsmp`, in its v2, v3 and v4 layouts) and Nord
Sample Editor projects (`.nsmpproj`) in one editor. An instrument holds encoded
audio; a project names WAV files and says how to lay them out.

What has been checked on hardware:

- Instruments encoded in the v2 layout have been played on hardware, in mono,
  stereo and looped.
- v3 and v4 instruments are fully supported, but playback of one drawbar has edited
  has not been tested on a real instrument. Encoding one is unverified: it reproduces
  the Sample Editor's own render, but no instrument that plays those layouts has
  played one.

The audio itself is never rewritten by an edit. Nothing is decoded to draw the
editor; a zone's audio is decoded only when you ask to hear or see it.

## The key map

The **Key map** stays pinned above the rest of the editor. Each zone is a band over
the keys it answers, and keys no zone answers are hatched. The heading reads
`every key answered`, or how many silent ranges there are.

Drag a band's edge to move it:

- **v2** stores only each zone's top note, and derives its low note from the zone
  below. Drag a top, and the band above follows.
- **v3 and v4** state both ends. Drag either edge; pull zones apart to leave keys
  silent, but they never overlap.

Where the keyboard map cannot be read, the bands cannot be dragged, and the editor
says so. The name can still be changed.

**Click a key to hear it.** The key is played at velocity 90, pitch-shifted from the
answering zone's root. The line under the keyboard says what answered:
`A2 at vel 90 → Zone 11 · root F#3 · shifted -9 st`. A key outside every zone, or
outside the answering zone's velocity window, is silent, and the line says why.

## Velocity

v3 and v4 zone records state a velocity window, drawn as a key-by-velocity field
under **Velocity**. It is read only: `nord-format` has no setter for it, and every
shipped instrument answers the full window. Click a block to open its zone.

## Zones

**Zones** lists one row per zone: the keys it answers, what its record states, its
length and channels once decoded, and its size. Click a row to open it.

- **Root key** and **Top note** are editable, and so is **Low note** where the layout
  states one. On v2 the low note is shown as *derived from the zone below*.
- **Velocity window** and **Gain** are shown read only, where the record holds them.
- **Show audio** decodes the zone and draws its waveform. **Play** and **Stop** then
  sound it, and **Save WAV…** writes it out as a WAV file.

## Per key

A v2 instrument also has a keyboard map with a gain and a detune for every key.
**Per key** draws them as two lanes over the keyboard: **Gain**, drawn to ±3.0 dB,
and **Detune**, drawn to ±25 c. Drag across a lane to paint values; they snap to a
twentieth of the lane, and values very near zero snap to zero. The heading shows the
instrument's own gain, which is read only.

**Show the 128-key gain and detune table** opens the same values as a table, to edit
one key at a time.

## Sound parameters

**Sound parameters** shows the loader's preset for the instrument's category, read
only.

## Metadata and Advanced

**Metadata** leads with the file's own facts: format, content version, the name
field and how much of it is used, the zone records, the sub name, and on v2 the
keyboard map.

**Advanced** is the capability table, **What this format holds**. Each row, such as
key zones, velocity layers or the per-key table, is **editable**, **read-only**,
**not in this format**, **needs an encode** or **verified**, with a note on why.
**Offsets** lists where each edited field lands in the file.

## Sample Editor projects

A project opens in the same editor, with these differences:

- Both ends of every band in the key map can be dragged.
- A struck key is not played: a project is built into an instrument before it plays.
- **Velocity** is editable. Drag the top or bottom edge of a zone's window.
- **Source strokes** lists the zones. An open row edits **Root key**, **Top note**,
  **Bottom note**, **Velocity window**, **Gain**, **Trim in → out**, **Loop in →
  length**, **Crossfade** and **Source file**.
- **Sound parameters** are editable: **Attack**, **Velocity → amplitude** and
  **Velocity → timbre**.

A project is saved back as the same text file the Sample Editor reads. It cannot be
built into an `.nsmp` yet, so its header action reads **Build → .nsmp** and does
nothing.

## From a WAV

A WAV opened on its own is not a Nord file, but it can become a one-zone sample
instrument. Its document offers:

- **Name**, and the zone's **root key** and **top note**;
- **Generation**: **v2**, **v3** or **v4**. v2 has been played on hardware; v3 and v4
  are unverified;
- **Plain records**, which stores every field outright rather than in the Sample
  Editor's own coding: the same audio in a larger file, and not the editor's bytes;
- **Encode**, which adds the new instrument to this computer and leaves the WAV as
  it is.

## New from WAVs

**New ▸ Sample instrument…** and **New ▸ Sample Editor project…** ask for WAV files,
then open a dialog with one row per file. WAVs dropped on the window while the dialog
is open join it.

- Set the **Name**, and a **root key** for each file. Files become zones, ordered
  by root key.
- **Sample instrument…** encodes the audio into a v2 instrument, so the WAVs are not
  needed afterwards. A WAV the encoder cannot take is refused before **Create**.
- **Sample Editor project…** stores the file names, and the Sample Editor looks for
  the WAVs beside the project, so keep them together.

**Create** makes the new document and opens it. **Cancel** leaves nothing behind.

To send an instrument to the keyboard, use the header's **Queue send**; see
[The instrument](instrument.md#the-send-queue). A sample over about a megabyte is
not kept between sessions; see
[Files on this computer](this-computer.md#what-is-kept-between-sessions).
