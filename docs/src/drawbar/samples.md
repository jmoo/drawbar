# Samples

drawbar edits sample instruments (`.nsmp`) and Nord Sample Editor projects
(`.nsmpproj`) in the same editor. An instrument carries its audio. A project
points at WAV files.

## The key map

The key map stays at the top. Each zone is a band over the keys it answers, and
keys nothing answers are hatched. Drag a band's edge to move it. The older v2
layout stores only each zone's top note, so dragging one moves its neighbour too.
The v3 and v4 layouts let you pull zones apart and leave keys silent.

Click a key to hear it. The line under the keyboard says which zone answered and
by how much it was shifted, or why the key is silent.

**MIDI**, above the map, listens to every controller on this computer, so you can
play the zones from a keyboard instead of clicking them. A key sounds at the
velocity you played it, which is the velocity a zone's window is tested against.
Nothing is sent to your instrument: a controller plays drawbar's own audition.

## Zones

One row per zone: the keys it answers, its length and channels, and its size.
Open a row to edit its root and top note, see its waveform with **Show audio**,
play it, or **Save WAV…**. Velocity windows are shown but cannot be edited in an
instrument.

A v2 instrument also has a gain and a detune for every key. **Per key** draws
them as two lanes you paint across, or as a table for one key at a time.

## Projects

A project opens in the same editor with everything editable: both ends of each
zone, velocity windows, trims, loops, crossfades, source files and the sound
parameters. It is saved back as the text file the Sample Editor reads. drawbar
cannot build a project into an instrument yet.

## From WAVs

Drop a WAV on the window and its document can **Encode** it into a one-zone
instrument. **New ▸ Sample instrument…** takes several WAVs, one zone each, with a
root key for each. **New ▸ Sample Editor project…** makes a project from them
instead, and the Sample Editor expects the WAVs to stay beside it.

Instruments encoded as v2 have been played on an instrument. v3 and v4 are
offered but unverified. See [What is supported](../getting-started/support.md).

## Sending

Queue an instrument from its header like anything else. Samples are usually
larger than what drawbar keeps between sessions, so export the ones you want to
keep.
