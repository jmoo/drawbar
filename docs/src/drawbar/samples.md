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
its own, with its rate, channels, sample format and length in the header.
drawbar reads PCM of 8 to 32 bits and 32- or 64-bit float, at 8,000 Hz or more.
drawbar-web opens WAVs of up to 120 MB; open a larger one in the desktop app. A
WAV drawbar cannot read, such as a compressed one, says why.

**Play** plays the file. **Gain**, set with **Apply gain**, is the file's total
gain against the file as last saved, and the line beside it says how many
strikes that gain would clip.
The file keeps its own format. Every change to a WAV is an edit: **Save** writes
it into the file and **Revert** takes it back.

### The take

A WAV recorded for a piano library is a take: notes played one after another,
which the file marks as hits. The waveform shows the whole take. Each hit is a
box around its regions, named by its note and layer, such as `C4 v22`. The
header's **Take** says how many hits, regions and loops the file marks, or
**raw: no regions**. The **+** and **−** right of the lane bend its scale toward
log, so quiet sound such as a release stands taller. **Zoom in**, **Zoom out**
and **Whole file** change the time shown; drag the lane or its scroll bar to
move along.

Click a box to pick its hit. Click where no box is to mark a place, such as a
strike the file does not mark yet.

**Detect** finds the take's hits as `nord piano prep` does: it names each by its
pitch, makes a note's hits its layers by level, and marks each hit's
attack, its loop or fade, and its release, in place of the take's regions.
**Skip hits** sets how far over the take's noise a hit must stand to be kept:
the build raises every layer to one level, so a quiet hit's hiss would rise with
it. **Build piano library…** opens **New ▸ Piano library** with the take as it
is now.

### The picked hit

Under the take, the picked hit is drawn close up, with a marker at each cut: the
start and end of its attack, loop and release, and where a fade starts. Drag a
marker to move its cut. A loop marker snaps to where the take rises through
zero, so the loop joins smoothly; hold Alt to place it exactly. **←** and **→**
pick the hit before and after it.

**Add** and **Delete** mark and unmark parts of the take; the audio is never
changed. **Add ▸ Hit** marks the strike at the place clicked, named by its pitch
and by how loud it is against the note's other layers. **Add ▸ Release** and
**Add ▸ Loop** give the picked hit a release or a loop to drag into place.
**Delete** removes the hit, its loop or its release. To rename a hit, type its
note and layer, such as `D4 v30`, in its name box.

### Hearing it as the instrument will

**Play original** plays the picked hit, or the place clicked, as recorded.
**Category** is the kind of piano the take is built as, and the build applies
its defaults. **Clean audio** takes out the take's hum and hiss as the build
does; with it off, the take is built as recorded. **Play hit** plays the hit as
the instrument will: hold it down as you would a key, and let go to hear its
release. **Loop wrap** plays round the loop, so the join is heard, and
**Release** plays the release alone. The fades are close to the instrument's,
not exact.

### Instruments from WAVs

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
