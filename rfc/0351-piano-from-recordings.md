# RFC-0351: A piano library from recordings

- Start date: 2026-11-20
- Authors: mikesprad
- RFC PR: [jmoo/drawbar#351](https://github.com/jmoo/drawbar/pull/351)
- Tracking issue: none yet

## Summary

Make **New ▸ Piano library (.npno)…** the one way into building a library from recordings. Its first
step asks what was recorded: one recording of the whole instrument, a few recordings (one per layer,
or one per range of keys), one recording per note struck, or finished strokes. drawbar detects the
hits across the whole set, shows them on a page of notes by layers, opens any of them in the take
editor, and builds. The set is kept as a piano project in the library, so the work can be closed and
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
import gave 50 WAV documents, each detected on its own.

## Guide-level explanation

**New ▸ Piano library (.npno)…** opens with one question:

```
What do you have?
( ) One recording of the whole instrument
( ) A few recordings: one per layer, or one per range of keys
( ) One recording per note struck
( ) Finished strokes, cut and looped

[Choose WAVs…]  [Choose folder…]
```

The answer is preselected from what is picked: one file is one recording, many short files are one
per note, and files named as strokes (`060-b0-l00.wav`) are finished strokes. It can be changed
before **Detect**. drawbar asks rather than decides because the shape tells the detectors where a
gain change or a new noise floor can begin, as RFC-0276 chose.

- **One recording** is today's take: Detect marks its hits, a note's hits its layers by level.
- **A few recordings** are either one per layer, each playing every note once with the loudest
  first, or one per range of keys, each a take of its own over its notes. A note found in two
  recordings of ranges is refused with both names, not merged. drawbar tells the two apart by
  whether the recordings play the same notes, and asks where it cannot tell.
- **One recording per note** is `nord piano prep`'s folder of one-note files: each file's note from
  its pitch, its layer from its level among its note's files.
- **Finished strokes** are built as they are, as New ▸ Piano library builds them today.

**Detect** runs across the whole set, then opens the project's page: the keyboard's notes across,
the layers down, each hit a cell. A cell shows the hit's level and whether it loops or fades. Under it,
the files left out are listed with the reason: too quiet, within the layer gap of a louder one, no
pitch found, or did not read. Clicking a cell opens that recording in the take editor with the hit
picked. Edits are made and saved there, as for any WAV, and the page shows them when it is next
shown. **Build** names, categorises and builds the library as New ▸ Piano library does now.

The project is saved in the library like any document, named by the user, with copies of its
recordings in a folder beside it, as a Sample Editor project's are. Opening it again shows the page
as it was left. Detecting again asks first, as the take editor's Detect does.

A WAV opened on its own keeps the take editor and **Build piano library…**, which makes a project of
that one recording.

## Reference-level explanation

### nord-format: a set of takes

`nord piano prep`'s set logic moves out of `nord-cli` into `nord_format::take::set`, so drawbar and the
CLI share it:

- `Shape { Take, PerLayer, PerRange, PerNote }`.
- `set::plan(takes: &[(name, Audio)], shape, &PlanOptions) -> SetPlan`: for each take its
  `plan::Plan`, and for each file left out the reason, as `prep` lists them today.
- `PerLayer` and `PerNote` are what `prep` does now with several takes and with a folder.
  `PerRange` plans each take on its own, as a single take, and refuses a note two takes both name.
- The CLI's listing, file walking and writing stay in `nord-cli`. `prep` gains `--ranges` for
  several takes that are ranges rather than layers.

### drawbar: the project

- A piano project is a small file in the library: its shape and its recordings' paths in the
  project's folder. The marks live in the recordings, as the take editor saves them, so the project
  holds nothing the WAVs already say.
- Its document is the page above. Detect runs `set::plan` in the background, as Detect does now,
  and writes each recording's marks.
- **Build** hands the recordings to the same build as New ▸ Piano library, whose takes already
  build together.
- On the web, **Choose folder…** shows where the browser can pick a folder, as the library picker
  does.

## Drawbacks

- A new document type and file in the library, kept for as long as drawbar reads it.
- Copying the recordings into the library doubles their space on disk. A project that pointed at
  them in place would break when they moved, and on the web there is no path to point at.

## Rationale and alternatives

- **Join the recordings into one long take** and reuse the take editor as it is. Rejected: each file
  has its own noise floor and, for one per layer, is its own layer. The joined take would have to
  carry both, and Detect would have to place the floors that the shapes exist to state.
- **A separate New entry per route.** Rejected: whoever recorded the instrument knows what they
  have, not what a stroke WAV is. One entry that asks sends them down the right route.
- **A one-off dialog, nothing saved.** Rejected: fixing 50 files is not a single sitting.
- **Doing nothing** leaves drawbar's piano building usable only by those who know the CLI.

## Prior art

- RFC-0276: the take editor, Detect, `nord piano prep` and its three take shapes.
- New ▸ Sample Editor project, which copies its WAVs into a new folder beside the project.
- `nord piano prep` with a folder or several takes, which built a library from a set of organ
  recordings before this RFC.

## Unresolved questions

- The project file's name and extension. It is drawbar's own, so it should not look like one of
  Nord's.
- Whether the page's grid should follow the existing keyboard view or be a plain table first.

## Future possibilities

- A retake of a few notes merged into a project.
- Rebuilding a library from the project it was built from (RFC-0276's link back to takes).
