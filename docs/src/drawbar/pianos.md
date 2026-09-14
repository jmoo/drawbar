# Pianos

A piano library (`.npno`) holds one recording, called a **stroke**, per root
note, bank and velocity layer. The editor is for choosing which strokes go to the
instrument, so that a library fits the memory it has free, and for setting how it
plays.

## Edits are a plan

An edit does not rewrite the library. It is a plan over the saved file, shown at
once, and every switch can be turned back on. The file is laid out only when it
has to be: on Save, Export, or Queue send. The header reads `applying…` while
that runs, and the action waits for it.

A plan is an unsaved edit like any other: the name is starred until it is saved,
and **Revert** drops it.

## The key map

Each root has a cell over the keys it answers, showing the megabytes it keeps.
Drag the boundary between two roots to move keys, and the outer ends to cover or
uncover keys. Click a key to hear which root answers it. Keys above the damper
limit are shaded.

## Trim to fit

A list of switches, each with the size it keeps: one per velocity layer, **Pedal
resonance**, **Release samples**, and **Keys**, which narrows the range to the
middle of the keyboard. Beside them, a meter compares the library with the free
piano memory the instrument reports, and a sentence names the cheapest cut that
would make it fit. A library that does not fit cannot be queued.

**Velocity layers** goes finer: one lane per layer, one segment per root. Click a
segment to drop that layer for that root only.

## Playback

**Instrument gain**, the damper limit (**Keys ring on above**), and **Kind**,
which is what the instrument files the library under. Kind changes nothing about
the sound.

**Roots** lists each root with its strokes. There you can trim a stroke in dB,
audition the root, save it as a WAV, or drop it. **Per key** paints a fine tune
across the keyboard.

## A new library

**New ▸ Piano library…** takes one WAV per stroke. A file named like
`060-b0-l00.wav` fills in its root (MIDI note 60), bank and layer for you.
Otherwise set them in the dialog. **Template** builds on a library already on
this computer, and the default builds from drawbar's own rules. Set the kind,
gain and damper limit in the new document afterwards.

Libraries drawbar has built or trimmed play on an instrument across every key.
Renames, retunes and remaps have not been played. See
[What is supported](../getting-started/support.md).

## Sending

Queue a library from its header. It is checked against the instrument and its
free space first. Libraries are far larger than what drawbar keeps between
sessions, so export what you want to keep.
