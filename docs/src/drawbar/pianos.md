# Pianos

drawbar opens Nord piano libraries (`.npno`). A library holds one recorded
**stroke** per root note, bank and velocity layer, and the editor is mostly for
deciding which of them go on the instrument: trimming a library to fit the memory
the instrument has free, and setting how it plays.

## Edits are a plan

An edit to a piano library does not rewrite the file. It is a plan laid over the
bytes last saved, and the editor shows the result at once. Every switch can be
turned back on.

The plan is laid out into a new file only when it has to be: on **Save** (⌘S),
**Export…**, or **Queue send**. That runs in the background. Meanwhile the header
reads `applying…`, the action reads **Applying…**, and the act you asked for waits
until the file is ready. Until then the header's state reads `edited`, or `trimmed`
when the plan drops strokes the saved file holds, and the size reads as what is kept
out of the whole.

A document holding a plan that is not yet laid out is kept when its tab closes.

## The key map

The **Key map** stays pinned above the editor. Each root has a cell over the keys it
answers, showing the megabytes it keeps. The heading reads `every key answered`, or
how many silent ranges there are.

- Drag the boundary between two roots to move keys from one to the other.
- Drag the outer end of the lowest or highest root to cover or uncover keys.
- Click a key to hear which root answers it. The line under the keyboard names the
  root, or says why the key is silent.

The damper limit is marked *damper stops here*; keys above it ring on after they are
released, and are shaded.

## Trim to fit

**Trim to fit** sets what is kept against what the instrument has free. Its left
column is a list of switches, each with the size it keeps:

- one switch per velocity layer, such as **Hard layer**, **Medium layer** and
  **Soft layer**. A layer switch speaks for every root;
- **Pedal resonance**, what the Small library leaves out;
- **Release samples**, the key-up tail;
- **Keys**, named with the library's whole range, such as **Keys A0 – C8**. Switch
  it off to keep only the middle of the keyboard, C2 to C7 at most.

Beside the switches, a meter shows the size in the file, the size kept, and the free
piano memory the attached instrument reports. The badge reads `fits · 12 MB to
spare`, `3 MB over`, or `no free piano memory reported`. Under the meter, a sentence
says whether the library fits and, if it does not, names the cheapest cut left that
would.

While a library does not fit, the header's action reads **No room · 3 MB over** and
the library cannot be queued.

## Playback

**Playback** holds what the instrument applies over every stroke:

- **Instrument gain**, from −12.7 to +12.7 dB;
- **Keys ring on above**, the damper limit. `none` damps every key;
- **Kind**, the kind of instrument the library is filed under, such as **Grand**,
  **Upright** or **Electric piano**. The instrument files the library under it; it
  changes nothing the library sounds like. Picking a kind also sets the damper limit
  to that kind's default, unless you have set one.

## Velocity layers

**Velocity layers** draws one lane per layer, with one segment per root. A segment
is labelled with the velocities that root's layer answers, and hovering it shows its
size and trim. Click a segment to drop that layer for that root only; click it again
to keep it.

## Roots

**Roots** lists every root with the keys it answers, its layers and its size. Open a
row to see the root key, the keys it answers, its channels and its fine tune, and a
line for each of its strokes: the bank and layer, a trim in dB you can change, and
the decay applied, in dB/s or `none`.

- **Audition** plays the root, and **Stop** stops it.
- **Save WAV…** writes the stroke Audition plays out as a WAV file.
- **Drop this root** drops every layer of the root from the plan.

Audio is decoded one stroke at a time, when you ask to hear it: the loudest attack
layer the plan keeps.

## Per key

**Per key** draws a fine tune lane over the keyboard, to ±25 cents. Drag across it
to paint values. **Show the 128-key fine tune table** opens the same values as a
table, to edit one key at a time.

## Metadata and Advanced

The header's name is two boxes, `Name` `#` `Variant`, as the file stores it.

**Metadata** leads with the file's facts: format, stream version, size before any
trim, name, channels, strokes and roots, keys answered, kind and gain.
**Advanced** shows the capability table and where each edited field lands in the
file.

## Sending a piano

A piano library is queued, dropped on a slot, copied, renamed, moved and deleted
like a sample. Before it is queued, it is checked against what the attached
instrument takes and against the free space in its Pianos folder. See
[The instrument](instrument.md#the-send-queue).

A library over about a megabyte is not kept between sessions; export it to keep it.
See [Files on this computer](this-computer.md#what-is-kept-between-sessions).

## A new piano library

**New ▸ Piano library…** asks for WAV files, one per stroke, and opens a dialog.
WAVs dropped on the window while the dialog is open join it.

- Set the **Name**. For each WAV, set its **root**, its bank, and its layer as an
  **index** or a **value**. A file named like `060-b0-l00.wav` sets root 60, bank 0
  and layer index 0 for you; `-v12` in place of `-l00` gives a layer value.
- Under **Advanced**, **Template** picks a piano library already on this computer
  to build on. The default is `none — the rules`.
- **Create** builds the library. If the build refuses, the reason is shown and your
  files stay in the dialog. Samples that clipped when resampled are reported in the
  activity log.

Kind, gain and the damper limit are set in the new document afterwards.
