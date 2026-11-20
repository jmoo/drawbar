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
editor as if it were one long WAV: the notes from lowest to highest, each note's strokes from loudest to
softest. The set is saved as a piano project in the library, so the work can be closed and picked up
later.

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

**New ▸ Piano project…** opens with one question:

```
What do you have?
( ) One recording of the whole instrument
( ) A few recordings: one per layer, or one per range of keys
( ) One recording per note struck
( ) Finished strokes, cut and looped
```

The WAVs are chosen as a folder or as files. drawbar asks for the shape rather than deciding it,
because the shape tells the detectors where a gain change or a new noise floor can begin, as RFC-0276
chose.

- **One recording** is today's take: its hits found, a note's hits its layers by level.
- **A few recordings** are either one per layer, each playing every note once with the loudest
  first, or one per range of keys, each a take of its own over its notes. A note found in two
  recordings of ranges is refused with both names, not merged.
- **One recording per note** is `nord piano prep`'s folder of one-note files: each file's note from
  its pitch, its layer from its level among its note's files.
- **Finished strokes** are kept as they are, and can still be checked, cleaned and adjusted.

A dialog then lists each WAV with what was found, for example:

```
Grand-01.wav    C1, layer 2 of 3
Grand-02.wav    no note found: added to the end
Grand-03.wav    too quiet: added to the end
Take-low.wav    detected 10 notes (C2 – C4) with 3 velocity layers each
Broken.wav      did not read: …
```

The rows only report; nothing is fixed there. **Create** saves the project and opens it.

The project opens in the take editor, as one long view of the whole set: the notes from lowest to
highest, each note's strokes from loudest to softest, and anything drawbar could not work out at the
end. Everything the take editor does on one WAV it does here: stepping through the hits, editing cuts
and loops, auditions, cleaning, and Build. A single WAV made into a project looks the same, as a set
of one.

The project is saved in the library like any document, with copies of its WAVs in a folder beside
it, as a Sample Editor project's are. Opening it again shows the long view as it was left.

A WAV opened on its own from the library is unchanged: it opens in the take editor, and **Build
piano library…** works as it does now.

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
- The CLI's listing, file walking and writing stay in `nord-cli`. `prep` gains `--ranges` for
  several takes that are ranges rather than layers.

### drawbar: the project

- A piano project is a small `.drawbar-piano` file in the library: its shape, its recordings' paths in
  the project's folder, and their order in the long view. The extension names it as drawbar's own, not
  one of Nord's.
- The marks live in each recording's copy in the library, as the take editor saves them now: written
  only on save, with only the cue, label and loop chunks rewritten and the audio and other chunks kept
  byte for byte. The files the recordings were copied from are never written. The project file holds
  nothing the WAVs already say.
- The long view is built in memory from the recordings and never written as a WAV. Each recording
  keeps its own noise floor and its own place as a layer. A save that changes several recordings
  asks for each of their saves at once, so the library takes them as one change and **Edit ▸ Undo**
  takes them back in one step.
- Finding the notes runs `set::plan` in the background, as Detect does now.
- **Build** hands the recordings to the same build as New ▸ Piano library, whose marked takes already
  build together.

## Drawbacks

- A new document type and file in the library, kept for as long as drawbar reads it.
- Copying the recordings into the library doubles their space on disk. drawbar keeps everything it
  works on in its library, and on the web there is no path to point at instead.
- The take editor's unsaved edit history covers one WAV today; the long view needs one that spans its
  recordings.

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

- The hint under each choice, and the New menu item's hover text.
- Whether the choice is preselected from what is picked (one file, many short files, files named as
  strokes).
- How the long view shows a set of hundreds of files: I'd like to show only each recording's hit
  (attack to release end) with a short gap between, which keeps it small on the web.
- What a set of recordings at different sample rates does: refused, naming the odd ones out, unless
  there is a better answer.
- What **Edit ▸ Undo** calls a save that changed many recordings.

## Future possibilities

- A retake of a few notes merged into a project.
- Rebuilding a library from the project it was built from (RFC-0276's link back to takes).
- A WAV opened on its own opening as a project of one, so every route works the same way.
