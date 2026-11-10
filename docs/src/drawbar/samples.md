# Samples

drawbar edits sample instruments (`.nsmp`) and Nord Sample Editor projects
(`.nsmpproj`) in the same editor. An instrument carries its audio. A project
points at WAV files.

## The key map

The key map stays at the top. Each zone is a band over the keys it answers, and
keys nothing answers are hatched. Drag a band's edge to move it. The older v2
layout stores only each zone's top note, so dragging one moves its neighbor too.
The v3 and v4 layouts let you pull zones apart and leave keys silent.

Click a key to hear it, or play the keys on a
[MIDI controller](overview.md#midi-controllers). The line under the keyboard says
which zone answered the last key and by how much it was shifted, or why the key
is silent. A played key sounds at the velocity you played it, which is the
velocity a zone's window is tested against.

## Zones

One row per zone: the keys it answers, its length and channels, and its size.
Open a row for its waveform, to edit its root and top note, to play it, or to
**Save WAV…**. Velocity windows are shown but cannot be edited in an instrument.

A v2 instrument also has a gain and a detune for every key. **Per key** draws
them as two lanes you paint across, or as a table for one key at a time.

## Projects

A project opens in the same editor with everything editable: both ends of each
zone, velocity windows, trims, loops, crossfades, source files and the sound
parameters. It is saved back as the text file the Sample Editor reads.

**Build → .nsmp** in the header builds the instrument a project describes. It
is offered once every WAV the project plays is in the library, at the path the
project gives from its own folder. A name that differs only in case counts.
Otherwise the header reads **N WAVs missing**, and hovering over it lists each
one and why: not in the library, outside it (an absolute path, or one that
climbs above the library's folder), matching two files whose names differ only
in case, or a hidden file, such as an AIFF, since drawbar builds only from
WAVs.

A build reads the project and its WAVs as drawbar holds them, unsaved edits
included, and tries again any WAV an earlier read could not read. It writes a
v2 instrument beside the project under the project file's name, numbered if
that name is taken. The instrument opens in a tab. Settings
the instrument cannot hold are listed in the activity log, and anything the
encoder cannot reproduce stops the build with the reason.

## WAVs

Drop WAVs on the window, or use **File ▸ Import…**, to copy them into your
library, where they are listed beside your sounds. A WAV opens as a document of
its own: the whole file's level, with its rate,
channels, sample format and length in the header. It reads PCM of 8, 16, 24 or
32 bits and 32- or 64-bit float, and plays at the file's own depth. **Play**
plays it. **Gain** is the file's total gain against the file as last saved:
**Apply gain** sets it, so 10 dB and then 12 dB is 12 dB in all, and 0 dB
gives the saved file back. The line beside it first says how many of the
file's notes that gain would clip. The file keeps its own format: a 24-bit WAV
stays 24-bit, and a float one never clips. A gain is an edit like any other: a
WAV closed unsaved opens again on its gain, **Save** writes it into the file,
which the next gain then starts from, and **Revert** takes it back. A WAV drawbar cannot read, such as a compressed one, says why and
offers neither.

The waveform is the take's lane: its level, the louder channel at each point, rising from a baseline. Each note the file marks is one box around its regions, named by its attack, such as `C4 v22` for `060-b0-v22` and `060-b2-v22`, read by their labels as `nord piano build` reads them; a region labelled any other way is boxed under its own label. The header's **Take** says how many hits, regions and loops the file marks, or **raw: no regions** for a WAV as recorded; the list calls a WAV with regions a **WAV (processed)**. Hover over a box for its regions' labels; click one to pick its note, which is outlined in the text color. **Zoom in** and **Zoom out** halve and double the time shown, down to 3 seconds; drag the lane, or the scroll bar under it, to move along the take; **Selected** shows the picked hit, and **Whole take** the whole file.

Under the take, the picked hit is drawn close up, from its first cut to its last
with a margin: its attack, loop, release and fade, and a marker at each cut
point (attack start and end, loop start and end, release start and end, and
where a fade starts). **Play hit** plays the picked note as recorded, from its first cut to its last. Hover over a marker for its name and time. **Zoom in**
and **Zoom out** go down to 50 ms, **Loop** shows the loop, and **Whole hit**
the whole hit again; drag the lane to move along. The markers show where the
file cuts; they cannot be dragged yet.

**New ▸ Sample instrument…** takes several WAVs, one zone each, with a root key
for each. **New ▸ Sample Editor project…** makes a project from them instead, in
a new folder named after it. Its WAVs are copied beside it, so Build and the
Sample Editor find them. Two WAVs with the same name are numbered apart, as
`c3 2.wav`.

The WAVs must be 16-bit PCM at 44.1 kHz, mono or stereo, in the plain or the
extensible WAV format. Convert anything else, such as a 24-bit or 48 kHz file,
first.

Instruments drawbar makes from WAVs are v2.

## Sending

Queue an instrument from its header like anything else.
