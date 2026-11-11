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
plays it. **Gain**, over the take, is the file's total gain against the file as last saved:
**Apply gain** sets it, so 10 dB and then 12 dB is 12 dB in all, and 0 dB
gives the saved file back. The line beside it first says how many of the
file's notes that gain would clip. The file keeps its own format: a 24-bit WAV
stays 24-bit, and a float one never clips. A gain is an edit like any other: a
WAV closed unsaved opens again on its gain, **Save** writes it into the file,
which the next gain then starts from, and **Revert** takes it back. A WAV drawbar cannot read, such as a compressed one, says why and
offers neither.

The waveform is the take's lane: its level, the louder channel at each point, rising in decibels from a baseline 72 dB under the take's loudest point, so quiet releases show, with a line every 12 dB of full scale. The small **+** and **−** right of the lane, at the top and foot of its scale, draw the scale deeper, to 96 or 120 dB, so the quiet end stands taller, or shallower, to 48 dB: the scale changes, not the gain, and the lines say where each level is. Each note the file marks is one box around its regions, named by its attack, such as `C4 v22` for `060-b0-v22` and `060-b2-v22`, read by their labels as `nord piano build` reads them; a region labelled any other way is boxed under its own label. The header's **Take** says how many samples, regions and loops the file marks, or **raw: no regions** for a WAV as recorded; the list calls a WAV with regions a **WAV (processed)**. Hover over a box for its regions' labels; click one to pick its note, which is outlined in the text color. Click where no box is, such as on a hit the file does not mark, and a line marks the place: the lane under the take shows from half a second before it to six seconds after, for **Add ▸ Note**. **Zoom in** and **Zoom out** halve and double the time shown, down to 3 seconds; drag the lane, or the scroll bar under it, to move along the take; **Whole file** shows the whole file again.

Under the take, the picked sample is drawn close up, from its first cut to its last
with a margin: its attack, loop, release and fade, and a marker at each cut
point (attack start and end, loop start and end, release start and end, and
where a fade starts). **Play original** plays the picked sample as the take holds it, from its first cut to its last, unshaped. Hover over a marker for its name and time. **Zoom in**
and **Zoom out** go down to 50 ms, **Loop** shows the loop, and **Whole sample**
the whole sample again; drag the lane to move along. Its **+** and **−** set its scale as the take's do.

**Add** and **Delete**, in the row over the picked sample, add and delete marks; the audio is never changed, and each is an edit like a moved cut, which Undo takes back. **Add ▸ Note**, at a place clicked in the take, marks a new sample: drawbar finds the hit nearest before the place, its key-up and its pitch, and names it by the pitch and by how loud it is against the note's other layers, a louder layer valued lower, as `nord piano build` reads them. The sample gets an attack from the hit to its key-up and a key-off after it, and is picked, ready to drag. Where drawbar finds no hit it starts at the place, where it finds no key-up the attack lasts 3 seconds, and where it finds no pitch the sample is named by the note of the sample nearest it, or as middle C. **Add ▸ Key-off** gives the picked sample a key-off from its key-up to where the take falls to near its noise, up to 2 seconds. **Add ▸ Loop** gives it a quarter-second loop three-fifths of the way into its attack, to drag into place. **Delete ▸ Note**, **Loop** or **Key-off** deletes that part of the picked sample's marks; deleting the note leaves its place marked. The small **←** and **→** at the far right of the row under it pick the sample before and after it in the take, bringing it into view. Drag a marker to move its
cut: both lanes follow it, and when you let go its region or loop is written to
the file's marks, one edit a drag, with the audio untouched. A marker stops at the
cuts either side of it, so a loop stays inside its attack and a fade ends with
it. Moved cuts are edits like a gain: **Save** writes them, **Revert** takes
them back, and a gain applied after them keeps them. **Undo**, beside the gain,
takes back the last edit, a gain or a moved cut, one at a time back to the
file as last saved; hover over it to see what it undoes. **Edit ▸ Undo** (⌘Z or
Ctrl+Z) and **Edit ▸ Redo** (⇧⌘Z or Ctrl+Shift+Z) work on one timeline, as in any
other app: Undo takes back the most recent change, whether it was to the library,
such as a rename or a save, or to the open WAV, and Redo makes again the change
undone last, until a new change is made. Typing in a box undoes its own text.

Under the picked sample, beside **Play original**, its controls play it as the
instrument will. **Category**, by the gain, is the take's category,
one of those a piano library files itself under, grand until you choose; the
build applies that category's defaults, and levels and fades can be changed
afterwards in the piano editor. **Clean audio**, on unless you turn it off,
cleans each sample of the take's hum and hiss as `nord piano build` cleans it;
the take's noise is found once, from when the take opens, which takes a few
seconds on a long take, and a control pressed before then says to wait. Both
are edits like a gain, kept in the WAV on **Save**, and the build follows them:
a take with Clean audio off is built as recorded. Click **Play sample** to hear
the whole sample: the attack, once round its loop, then the key-off. Hold it
down as you would hold a key: the attack plays shaped as `nord piano build`
shapes it, with its loop evened out and crossfaded, the note's loudest layer at
its recorded level, as Play original plays it, and a softer layer half its
recorded gap under it,
fading from the start as a library of that category fades, then round its loop,
fading as that category's loops fade, until you let go. Let go, and it fades
away as a key-up fades it, under its release, at the balance the build plays the
release at; a key above the category's damper rings on instead. **Loop wrap**
plays from a second before the loop's end and round the loop, at one level, so
the join is heard and not the fade; click it again to stop. **Release** plays
the release alone. The fades are the ones a library built from rules states: the
key-up fade is confirmed on the instrument, and each category's rates are fitted
to vendor libraries, so take them as close, not exact.

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
