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
its own: the whole file's waveform, one lane per channel, with its rate,
channels and length in the header. **Play** plays it. **Gain** with **Apply
gain** scales every sample by that gain, and the line beside it first says how
many samples would clip. A gain is an edit like any other: **Save** writes it
into the file and **Revert** takes it back. A WAV drawbar cannot read, such as
a 24-bit one, says why and offers neither.

**New ▸ Sample instrument (.nsmp)…** takes several WAVs, one zone each, with a
root key for each. **New ▸ Sample Editor project (.nsmpproj)…** makes a project
from them instead, in a new folder named after it. Its WAVs are copied beside
it, so Build and the Sample Editor find them. Two WAVs with the same name are
numbered apart, as `c3 2.wav`.

The WAVs must be 16-bit PCM at 44.1 kHz, mono or stereo, in the plain or the
extensible WAV format. Convert anything else, such as a 24-bit or 48 kHz file,
first.

Instruments drawbar makes from WAVs are v2.

## Sending

Queue an instrument from its header like anything else.
