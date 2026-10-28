# Pianos

A piano library (`.npno`) holds one recording, called a **stroke**, per root
note, bank and velocity layer. The editor is for choosing which strokes go to the
instrument, so that a library fits the memory it has free, and for setting how it
plays.

## Edits are a plan

An edit is a plan over the saved library, shown at once, and every switch can
be turned back on. The library is rewritten only when you save, export or queue
it, and that can take a moment for a large one. Until then the plan is an
unsaved edit like any other: the name is starred, and **Revert** drops it.

## The key map

Each root has a cell over the keys it answers. Drag the boundary between two
roots to move keys, and the outer ends to cover or uncover keys. Click a key to
hear it, or play the keys on a [MIDI controller](overview.md#midi-controllers).
A key sounds the loudest layer the plan keeps, however hard you play it.

## Trim to fit

A list of switches, each with the size it keeps: one per velocity layer, **Pedal
resonance**, **Release samples**, and **Keys**, which narrows the range. A meter
compares the library with the instrument's free piano memory and names the
cheapest cut that would make it fit. A library that does not fit cannot be
queued.

**Velocity layers** goes finer: one lane per layer, one segment per root. Click a
segment to drop that layer for that root only. Each segment gives the velocities
its layer plays at with the program's KBD Touch at 0. A higher KBD Touch reaches
each softer layer with a softer note.

## Playback

**Instrument gain**, the damper limit (**Keys ring on above**), and **Kind**,
which is what the instrument files the library under. Kind changes nothing about
the sound.

**Roots** lists each root. Open one to trim its level, hear it, save it as a
WAV, or drop it. A 6.x library trims in tenths of a decibel and can also boost a
stroke, by up to about 10.8 dB together with the instrument gain. A 5.x library
trims in whole decibels and cannot boost. **Per key** paints a fine tune across the keyboard.

## A new library

**New ▸ Piano library…** takes one WAV per stroke: 16-bit PCM, mono or stereo,
at any sample rate. A file named like `060-b0-l00.wav` fills in its root (MIDI
note 60), bank and layer for you. Otherwise set them in the dialog. **Template** builds on a library you already
have.

A new library is written in piano format 6.x. **Advanced ▸ Format** offers 5.x
for an instrument that does not play 6.x. With a template, the library takes the
template's format. A layer's value is on its format's scale: 0 to 135 spreads a
root over the velocity range in 6.x, and 0 to 27 in 5.x.

A WAV with a loop, which most sample editors can set, makes a stroke that loops
there while the key is held, and the dialog marks it. Audio after the loop's end
is not used, and the loop's last four samples are replaced by the four just
before its start, which a seamless loop already has. While the key is held, the
loop fades the way the instrument's own grand pianos do, faster on higher keys;
with **Template** it fades as the template's does, or as pianos of the template's
kind do when the template itself never loops. Without a loop, a stroke plays
to its end. Release strokes cannot loop.

[What is supported](../getting-started/support.md) says which of these edits
have been played on an instrument.

## Sending

Queue a library from its header. It is checked against the instrument and its
free space first.
