# RFC-0351: A piano library from recordings

- Start date: 2026-11-20
- Authors: mikesprad
- RFC PR: [jmoo/drawbar#351](https://github.com/jmoo/drawbar/pull/351)
- Tracking issue: none yet

## Summary

Add **New ▸ Piano project…**, a way into building a piano library from recordings, next to **New ▸
Piano library (.npno)…**, which stays as it is. It asks what was recorded: one recording of the whole
instrument, a few recordings (one per layer, or one per range of keys), one recording per note struck,
or finished strokes. drawbar finds each recording's notes and levels and shows the whole set in the take
editor as if it were one long WAV: each recording whole, ordered by its lowest note and then its
loudest layer. The set is saved as a piano project in the library, so the work can be closed and
picked up later.

## Motivation

RFC-0276 built the take editor and `nord piano prep`, but in drawbar they can be reached only by
knowing where to look:

- The take editor is what a WAV opens as. Nothing in the New menu says so, and **Build piano
  library…** shows only once the WAV is open.
- **New ▸ Piano library** says it "encodes one WAV per stroke". A long take picked there with no
  regions is read as a single stroke on one key, with no word that it should be detected first.
- A folder of one-note recordings, the way most sample sets come, can be prepped only by `nord piano
  prep`. Detect in drawbar sees one file at a time, so it cannot place a note's files as layers by
  level: every file's single hit is named the same layer, and the build refuses them.
- The same goes for one take per velocity layer, which `prep` handles and drawbar does not.

Trying it on a set of 50 one-note WAVs, I found no way to get from them to a library in drawbar:
import gave 50 WAV documents, each detected on its own, and no one view to check them in.

## Guide-level explanation

**New ▸ Piano project…**, whose hover reads "prepares WAVs for a piano library; copies them into a new
folder", opens the same file picker as **New ▸ Piano library**, and drawbar reads the WAVs picked. A
single WAV is one recording of the whole instrument, and nothing is asked; the dialog's caption says
"One recording of the whole instrument: its notes and layers are found from its hits." For several,
the caption says "The WAVs are copied into a new folder with the project. Nothing is changed in the
originals.", and the dialog asks what they are, offering only what several WAVs can be:

```
What do you have?
( ) A few recordings: one per layer, or one per range of keys
    each a take of many notes: every note at one level, or some of the keys at every level
( ) One recording per note struck
    each WAV holds one note, struck once; its level sets its layer
( ) Finished strokes, cut and looped
    named like 060-b0-l00.wav; kept as they are, to check and clean
```

drawbar picks its best guess from what it read: finished strokes where the files are named as strokes
or already marked, one per note where each holds one hit, a few recordings where each holds many.
Choosing another reads the rows again, so a wrong guess shows at once. drawbar asks for the shape
rather than deciding it outright, because the shape tells the detectors where a gain change or a new
noise floor can begin, as RFC-0276 chose. Whether a few recordings are layers or ranges is not asked:
takes over the same notes are layers, and takes over different notes are ranges.

- **One recording**, a single WAV, is today's take: its hits found, a note's hits its layers by level.
- **A few recordings** are either one per layer, each playing every note once with the loudest
  first, or one per range of keys, each a take of its own over its notes. A note found in two
  recordings of ranges is refused with both names, not merged.
- **One recording per note** is `nord piano prep`'s folder of one-note files: each file's note from
  its pitch, its layer from its level among its note's files. Every file with a hit becomes a layer of
  its note: near-equal levels can still differ in tone, so none is dropped for its level. Where the
  files are numbered as one series and most pitches rise evenly with the number, a pitch that breaks
  the series takes the note the series puts there (a top note's hammer knock, or a low note's missing
  fundamental, is otherwise read as another note).
- **Finished strokes** are kept as they are, and can still be checked, cleaned and adjusted. Each is
  marked whole from its name when the project is created, keeping its loop, so it shows at once.

A dialog then lists each WAV with what was found, for example:

```
Name [Grand]
Grand-01.wav    C1, layer 2 of 3
Grand-02.wav    no note found: left unmarked
Grand-03.wav    too quiet: left unmarked
Take-low.wav    detected 10 notes (C2 – C4) with 3 velocity layers each
Broken.wav      did not read: …
```

The rows only report; nothing is fixed there. The name, prefilled from the first WAV's, names the
project and its folder. **Create** copies the WAVs into that folder, saves the project and opens it. A
set whose WAVs differ in rate or channel count is refused before Create, naming two of them.

The project opens in the take editor, as one long view of the whole set. Each recording is shown
whole, once, so an edit can reach anything in the file; a take's hits stay together in recorded order.
Recordings are ordered by their lowest marked note, then their loudest layer. A recording with no hit
found sits where its name puts it when the set is named in sequence (after the note its name states,
or after the recording numbered before it), and otherwise at the end. Each WAV's name is shown over
its stretch, faint where it is unmarked. A **Sort by note** button reorders the view by the current
marks after notes are renamed, and then reads **Original order** to go back.

Everything the take editor does on one WAV it does here: stepping through the hits, editing cuts
and loops, auditions, cleaning, and Build. The hit's **Add** and **Delete** menus add and delete a
hit, its loop, its fade or its release; a hit loops or fades, never both. A hit added where Detect
found none is named by its pitch, or, where that can't be read, by the note found for its recording.
A single WAV made into a project looks the same, as a set of one.

In the library the project shows as a "piano project": recordings with their notes, layers, loops and
key-offs marked, ready to build a piano library. It is saved there like any document, with copies of
its WAVs in a folder beside it, as a Sample Editor project's are. Opening it again shows the long view
as it was left.

A WAV opened on its own from the library is unchanged: it opens in the take editor, and **Build
piano library…** works as it does now.

One string of **New ▸ Piano library** changes. Its hover says "encodes one WAV per stroke", but it also
builds each marked hit of a take, and takes a recording nobody has marked as a single stroke on one
key, with no warning. It becomes "builds from finished strokes or marked takes; for raw recordings use
Piano project…".

## Reference-level explanation

### nord-format: a set of takes

`nord piano prep`'s set logic moves out of `nord-cli` into `nord_format::take::set`, so drawbar and the
CLI share it:

- `Shape { PerLayer, PerRange, PerNote }`; one take on its own is planned as now.
- `set::plan(count, shape, &PlanOptions, skips, read)`: for each take its `plan::Plan`, or why it did
  not read, and for a one-note take where it was placed. Each take is read through `read`, so none is
  held with the others.
- `PerLayer` and `PerNote` are what `prep` does now with several takes and with a folder.
  `PerRange` plans each take on its own, as a single take, and refuses a note two takes both name.
- `PerNote` reads the files' names: a near-equal take of one named series is kept as a layer, and a
  pitch off an evenly numbered series' line takes the line's note.
- The CLI's listing, file walking and writing stay in `nord-cli`. `prep` gains `--ranges` for
  several takes that are ranges rather than layers.

### nord-format: detection and build

Working through real sets in the long view showed where the detectors and build fell short. These
change what any take builds into, not only a project's:

- **Loops.** Detect and Add ▸ Loop run a scored search: lengths of whole periods from 0.5 to 5 s, ends
  no later than where the note sinks to 20 dB over the floor, scored on the waveform match at the
  join, the level step and the lift, with a later start preferred, since a quieter stretch hides a
  join's faults. The build's join crossfade is 150 ms (was 30 ms). Settings were chosen by ear over
  eight listening rounds on piano, Wurlitzer and organ.
- **Key-ups in one-note files.** A recording of one note finds its key-up as a damper's fall (steeper
  than the decay before it, from well over the floor, staying down), so beating dips and hiss are not
  taken for key-offs. A hit added there gets a release only where a damper fell.
- **Note-off fade.** Each attack's note-off fade slopes by note per category, as measured vendor
  libraries do (Upright 50 to 35 from A0 to C8, Grand 35 to 26, Wurlitzer 28 to 19), in place of 32 on
  every attack.
- **Releases** are balanced against their attack only within one recording; a release recorded apart
  from its attack plays 18 dB under its note's loudest attack, as New ▸ Piano library's stroke WAVs do.

### drawbar: the project

- A piano project is a small `.drawbar-piano` file in the library: its shape, its recordings' paths in
  the project's folder, and their order in the long view. The extension names it as drawbar's own, not
  one of Nord's.
- The marks live in each recording's copy in the library, as the take editor saves them now: written
  only on save, with only the cue, label and loop chunks rewritten and the audio and other chunks kept
  byte for byte. The files the recordings were copied from are never written. The project file holds
  nothing the WAVs already say.
- What the whole set is built with is the project's, not each recording's: one category, one choice
  of cleaning and one "Don't decay long notes", since they all become one `.npno`. They are kept in
  the project file and applied when the library is built. **Build piano library…** from a project
  starts on the project's category. The project file also keeps the long view's order.
- The set's gain is the library's own Instrument gain, the `.npno` field the Nord applies in 0.1 dB
  steps, as suggested in this PR, not a scaling of the audio before the build: a format 6 build
  already raises every stroke to one peak before it is rounded to 16 bits, so a quiet recording with
  headroom loses nothing. No recording's audio is rewritten for gain, unlike a gain on a WAV opened on
  its own today.
- The long view is built in memory from the recordings and never written as a WAV. Each recording
  keeps its own noise floor and its own place as a layer, and a region is held to the recording it
  lies mostly over. An edit reads and rewrites only the recordings whose marks it moved. A save that
  changes several recordings asks for each of their saves at once, so the library takes them as one
  change and **Edit ▸ Undo** ("Undo Save 3 files") takes them back in one step.
- Finding the notes runs `set::plan` in the background, as Detect does now.
- **Build** hands the recordings to the same build as New ▸ Piano library, whose marked takes already
  build together.

## Drawbacks

- A new document type and file in the library, kept for as long as drawbar reads it.
- Copying the recordings into the library doubles their space on disk. drawbar keeps everything it
  works on in its library, and on the web there is no path to point at instead.
- The take editor's unsaved edit history covers one WAV today; the long view needs one that spans its
  recordings. Until then Ctrl+Z in the long view reaches the library's undo. It will record its own
  edit steps on #356's undo once that lands.

## Rationale and alternatives

- **Join the recordings into one long WAV** and reuse the take editor as it is. Rejected: each file
  has its own noise floor and, for one per layer, is its own layer, and a joined file would have to
  carry both. The long view gives the same single place to work without writing a new WAV.
- **A page of notes by layers** for the project. Rejected: the piano document already shows a built
  library's notes by layers, and one more page makes drawbar harder to learn.
- **Asking the question inside New ▸ Piano library.** Rejected: it would change a dialog that works
  for what it does today. A separate entry leaves it as it is.
- **A one-off dialog, nothing saved.** Rejected: fixing 50 files is not a single sitting.
- **Doing nothing** leaves drawbar's piano building usable only by those who know the CLI.

## Prior art

- RFC-0276: the take editor, Detect, `nord piano prep` and its three take shapes.
- New ▸ Sample Editor project, which copies its WAVs into a new folder beside the project.
- `nord piano prep` with a folder or several takes, which built a library from a set of organ
  recordings before this RFC.

## Unresolved questions

- Whether the `.drawbar-piano` file becomes a slim prep file naming its takes through #360's
  "sources" relation, as discussed in this PR.

## Future possibilities

- A retake of a few notes merged into a project.
- Rebuilding a library from the project it was built from (RFC-0276's link back to takes).
- A WAV opened on its own opening as a project of one, so every route works the same way.
