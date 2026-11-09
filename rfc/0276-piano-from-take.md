# RFC-0276: Build piano libraries from a recorded take

- Start date: 2026-10-28
- Authors: mikesprad
- RFC PR: [jmoo/drawbar#276](https://github.com/jmoo/drawbar/pull/276)
- Tracking issue: none yet

## Summary

Turn one long recording of an instrument into the strokes of a piano library, with the recording
itself as the document. The take is first resampled to 35,002 Hz as a new WAV. Detectors then find
the hits, name their notes and layers, mark each attack and release, and find a sustain loop. On
Save, the results go into the WAV as standard regions and loops, labelled with stroke names. New ▸
Piano library then builds from "a WAV, or a named region of one". Levelling, fades, hum removal and
the loop crossfade are build rules. The processing goes into a new crate of pure functions over
sample buffers, which also takes over all WAV reading and writing. It is first exposed as a
`nord-cli` command and then as a drawbar UI. A Python prototype already does all of these steps;
libraries built from its output play on a Stage 4.

## Motivation

A piano library starts as one long recording. Each note is played at several strengths, held,
released, and followed by a gap. New ▸ Piano library takes one finished WAV per stroke. Today every
stroke has to be cut, leveled, looped and named in other software first. For a modest library of
13 roots × 3 layers plus releases, that is 66 files edited by hand. It is also the step where most
mistakes happen: the wrong layer order, key noise before the strike, a fade inside the loop, or
releases too quiet to hear.

None of the general-purpose audio tools knows what a stroke needs. The attack has to start on the
strike, the release has to come from the same hit's key-up, the loop has to end before the damper,
and the levels must follow the library's rules rather than each file's. Nord's own Sample Editor
handles key zones but not velocity layers, and it does not export WAVs.

The prototype shows the gap can be closed. A Wurlitzer recorded through an interface (8.5 minutes,
91 hits, 23 roots, 2 to 4 layers per root) and an earlier phone recording of the same instrument
(13 roots, 3 layers) each went from one WAV to a library that plays on a Stage 4 in a single run.
Hit splitting, note naming, layer order, cuts and loops came out right without hand editing; a sheet
listing the notes and a few hits to drop was the only input.

## Guide-level explanation

Recording stays outside drawbar. Anyone recording an instrument already has an audio interface and
software that records, so drawbar starts from the WAV. WAVs are documents in drawbar, as Nord files
are, and the take editor follows the other document editors.

1. **Open a take** (16-bit, 24-bit or 32-bit float). A take comes in one of two shapes, so the
   detectors never have to guess where a gain change or a new noise floor begins:
   - **One take**, each note played at every velocity in a row, loud to soft or in any order.
   - **One take per velocity layer**, each playing every note once, loudest take first. Each take is
     one layer whatever its level, so the gain may be raised for the softer ones, and each take's
     floor and hum are measured on their own. A note played twice in a take keeps its last hit.
2. **Resample.** Detect, the loop markers and Build work only at 35,002 Hz. At any other rate the
   document says so and offers Resample, which writes a new WAV next to the take and opens it. The
   original is never changed. The new WAV keeps the take's bit depth (24-bit or float); the build
   goes to 16-bit.
3. **Detect.** The notes in playing order: a list (`E3 G3 A#3 …`), or a start note, a step in
   semitones and a count. For a single take, the number of layers, and how they are chosen; takes of
   one layer each take their layer from their order. These are inputs to the detectors, not stored;
   Detect runs only when pressed, writes regions and is done. Pressed again on a take with regions,
   it first warns that it replaces them, and the user can cancel or go ahead:
   - **Playing order**: the first hit of a note is the loudest layer. This suits players who play
     loud, medium, soft, and recordings whose level can't be trusted (a phone's automatic gain made
     measured loudness useless on our test take).
   - **Loudness**: hits sorted by measured level, with near-duplicates within 2 dB dropped and the
     kept layers spread evenly in dB.
4. **Hit grid.** One row per note, one column per hit. Each cell is one hit, saved as two regions
   (attack and release). It plays the hit and shows its layer, its length, whether a release was
   found, and the loop match. Warnings mark a pitch that disagrees with the note list, a hit with no
   key-up, a hit too quiet to use, and a weak loop match. Each edit changes the WAV's regions:
   - drop a hit (a stray noise, a stopped note): delete its regions; Undo brings them back;
   - set its layer, swap it with another hit, or give it a different note: relabel the regions;
   - drag its start, key-up, release end and loop on the waveform: move the regions or loop. Loop
     points snap to zero crossings, and a loop-wrap audition plays from just before the loop end and
     lets it cycle.
5. **Save** with drawbar's global save. Edits are held until then and written in one write, which
   #226 makes one undo step.
6. **Build.** New ▸ Piano library takes the WAV and builds a stroke from each labelled region, so
   template, kind, gain and format work as they do now. It renders a fresh npno each time.

The editor uses the standard document top bar for the file name, Detect and Build. The left bar
stays the file tree. The selected hit, its cut points and its audition sit in the right sidebar.
While the take editor has focus, its cards sit above the global Selection and Tags cards, which
stay collapsed. The cards follow the hit selected in the take view; selecting another file in the
browser gives the sidebar back to that file.

The WAV is the record. A user decides only where each stroke sits, what it is, and where its loop
is, and standard RIFF chunks hold all three:

- a `cue ` point and a `LIST`/`adtl` `ltxt` region per stroke, labelled with the name the build
  already reads (`060-b0-l00` for the attack, `060-b2-l00` for the release);
- a `smpl` loop tied to the attack's cue ID, for a note that sustains;
- for a note that dies before its key-up, a region at the end of its attack labelled
  `060-b0-l00-fade`, where it starts fading to silence.

A save rewrites only the `cue `, `LIST`/`adtl` and `smpl` chunks. The audio and every other chunk
are kept byte for byte, so another app's data is never dropped. Nothing else is stored: no sheet, no
hit states, and no overrides to re-match when a detector changes. Other editors can read and write
the same regions.

| Step | Today | With this RFC |
|---|---|---|
| Split a long take into hits | — | Onset detection that skips clicks and handling noise |
| Name each hit's root | `060-b0-l00.wav` names or the dialog | From a note list in playing order, checked against detected pitch |
| Choose velocity layers | `lNN` or `vNN` names | By playing order or measured loudness, with per-hit override |
| Cut the attack | Whole WAV | Strike detection, key-up detection, length cap |
| Cut the release | Separate WAV by hand | From the key-up of the same hit, kept only when it stands clear of the noise |
| Level | Per-WAV gain (#165) | Build rules from the audio (#279); releases keep their recorded balance with their attack |
| Loop | From a WAV's `smpl` loop (#256) | Loop search, written as a `smpl` loop on the region; crossfade at build; covers #257 |
| Background noise | — | Too-quiet hits flagged; hum removed and hiss followed down per stroke at build |
| Sample rate | Resampled at build | Resample the take first, as a new WAV |
| Layer values, thresholds, trims, build | New ▸ Piano library (#267) | Builds from labelled regions as well as whole WAVs; v6 defaults in #278 and #279 |

## Reference-level explanation

Provenance uses the forms in `CONTRIBUTING.md`. "Heard on a Stage 4" means a by-ear check on our
instrument (Stage 4, OS 1.64), loading the library through Sound Manager.

### Input and resampling

- The take is read as 16-bit, 24-bit or 32-bit float PCM. A recording straight from an interface is
  normally 24-bit or float; #165's reader takes only 16-bit. drawbar's WAV document reads all
  three, converts to floating point for the waveform and the player, and keeps gain edits in the
  file's own format. Only the build goes down to 16 bits.
- Resample writes a band-limited resample to 35,002 Hz at the take's own bit depth (24-bit, or float
  for a float take) as a new WAV. Detection and loop search run on that, so the loop points are
  where the build plays them. On the interface take, detecting at 35,002 Hz rather than the take's
  44.1 kHz gave 146 strokes for 148: one key-up and one release fell just the other side of their
  thresholds. That is accepted.
- The detectors read the mean of a stereo take's channels. Its strokes keep both channels, with one
  loop for both.

### Hits

- Envelope: RMS in 10 ms frames, after a 15 Hz highpass, on the take as recorded.
- Onset: the envelope rises 15 dB above its minimum over the previous 150 ms and is still 15 dB above
  the noise floor (the 10th-percentile level) 100 ms later. The second test rejects clicks. Of onsets
  closer than 1 s, the loudest wins. Only the first frame of a rise counts; otherwise a note whose
  level wavers drifts the onset 30 to 50 ms late.
- Level of a hit: the envelope's maximum over its first 100 ms.
- Pitch of a hit: harmonic product spectrum over 50 to 550 ms after the onset.
- Too quiet: a hit whose peak is less than 54 dB over the hum-free floor is flagged, and Detect leaves
  it out. Levelling brings its hiss up with it: soft treble hits recorded about 30 dB over the floor
  gave very noisy loops (heard on a Stage 4).

### Notes and layers

- Hits are assigned by walking the note list in order. A hit moves on to the next note when its pitch
  class is nearer that note's than the current one's. Dropped hits take no part. Comparing pitch
  class tolerates the octave errors common on low notes.
- A hit more than half a semitone from its note's pitch class gets a warning.
- More hits than layers: loudness mode keeps the hits nearest evenly spaced levels between the
  loudest and softest; playing-order mode keeps the first N.
- The number of layers can differ per range of roots, for example 4 layers over the middle octaves
  and 3 elsewhere.
- Layer values are named into the stroke names, and the build turns them into thresholds as it does
  now. See Unresolved questions for the default spread.

### Cuts

- Strike: the first sample within −12 dB of the hit's early peak, minus a 1 ms pre-roll, found on the
  take before any filtering. Vendor attacks reach −6 dB within 1 ms (inferred from specimens). Key
  noise kept before the strike delayed the attack, more so on keys below the root (heard on a
  Stage 4). Finding the strike after hum filtering moved it and left a click (heard on a Stage 4).
- Key-up: the earliest 0.2 s window whose fall is within 3 dB of the steepest fall of at least 10 dB,
  starting while the note is 15 dB above the noise floor, searched from 0.3 s after the onset. The
  damper's thump can delay that window, so the slow decay over 0.4 to 0.15 s before it is fitted with
  a line. The key-up is where a line through the fall crosses it, found from the first frame that
  leaves the decay line by 3 dB for 3 frames running. On synthetic takes with known key-ups (6 seeds,
  102 key-ups) the error is −20 to 0 ms; the plain steepest-window version was 30 to 260 ms late.
- Without a key-up, the attack ends where the note sinks to 6 dB above the floor, and the hit has no
  release.
- Attack length is capped at 6 s by default. Vendor attacks are at most about 6.5 s, the lowest notes
  longest (inferred from specimens).
- Release: from 5 ms before key-up to where it reaches 6 dB over the quiet just before its hit,
  searched from the key-off's peak, at most 1 s. At build it fades in over 5 ms and out over at most
  40 ms. Against the take-wide floor with its hum removed, key-offs ran on 100 to 500 ms into hum and
  hiss. A release is kept when its key-off peak stands at least 10 dB over that quiet, or the note is
  at least 25 dB over the floor at key-up, so a damper's thump on a note that has nearly died is kept.
  Releases with neither were mostly hiss, and levelling made them buzz (heard on a Stage 4). Vendor
  releases run 0.5 to 0.9 s (inferred from specimens).
- At build each attack fades in over 0.3 ms, so it never starts on a step.
- No fade at the end of a looped attack: the end sits inside the loop. A fade there produced a loud
  square wave or fast clicking once the loop played (heard on a Stage 4).

### Keys past the take

- A root plays from 12 semitones under it to one over. In a format 6 library built from rules, the
  build pitches the lowest root's strokes down, and the highest's up, to roots that reach the 88 keys
  A0 to C8, by at most 12 semitones each, as the prototype did. A stroke is pitched by the ratio of
  denominator at most 2000 nearest its interval, after cleaning and before its loop or fade is
  shaped, which are moved with it.

### Level

- Vendor libraries keep every layer near the same peak (inferred from specimens: attack layers peak
  at −12.5 to −17.5 dBFS), and the playback gain does the rest.
- A format 6 library built from rules (#279) plays each stroke at its root's loudest layer's level,
  whatever the stroke's own level, and each root at its loudest layer's audio level. So before
  rounding to 16 bits, the build brings every region of a take to one peak, −13 dBFS, as the
  prototype did. #279 levels each layer against its root's loudest, but not one root against
  another. Kept at their recorded levels, the treble roots of the interface take played 2 to 12 dB
  under the prototype's, and the top end sounded quiet and lifeless (heard on a Stage 4); brought
  to one peak, every attack plays within 0.3 dB of the prototype's. Unraised, the releases held 5
  to 6 bits, needed more boost than the instrument gives, and so turned the whole library down
  24 dB. Now a release that needs more boost than the instrument gives plays quieter, at the most
  boost there is, and only an attack or resonance stroke turns the library down. A format 5 library, or one from a template, plays each stroke's own level, and nothing is
  changed.
- Releases from a take keep their recorded level under their own attack (the region of their root at
  their layer), lifted by at most 12 dB and never louder than 14 dB under it. A release from a
  directory of WAVs keeps #279's fixed trim, since each WAV may have been levelled on its own. Both
  levels are measured on the strokes as the build cleans them: on the raw take, hum and hiss read the
  quiet treble key-offs of the interface take about 10 dB louder than they are. Cleaned, every
  release from F#4 up plays within 0.1 dB of the prototype's balance less its 1 dB offset, and those
  below within a few dB. Scaled with their attack's gain alone, they sat 35 to 57 dB down and could not
  be heard. Levelled all to one peak, the quiet key-offs were lifted 25 to 40 dB and their noise came
  up with them (heard on a Stage 4). Trims up to about 49 dB under the attack have played fine (heard on a Stage 4).

### Loop

Two cases, decided by how the note decays before its key-up:

- **Dying notes** come within 20 dB of the floor before the damper, 150 ms before the key-up, and
  would finish fading before it: mostly short treble notes. They get no loop. Detect writes a fade
  region from where the note came that near the floor to where its fade reaches silence, and the
  attack ends there. At build the stroke follows the note's own decay rate, fitted over the 400 ms
  before, slowing to a third of it, down to 70 dB under its peak, then loops on 100 ms of silence.
  The fade only lowers the audio, so the noise goes down with it. A loop placed in such a note sat in
  the noise and clicked, and a straight line in dB was heard as a fade that stops (heard on a
  Stage 4). Vendor dying notes also loop on silence, 48 to 63 dB under their peak (inferred from
  specimens).
- **Sustaining notes**, every other one, held ones included: a loop of about 500 ms, ending 150 ms
  before the key-up, clear of the damper, or earlier where the note sinks to 30 dB over the floor.
  Its length is a whole number of the note's periods, tuned within one period either side for the
  best normalized correlation between the audio before the loop end and the audio before the loop
  start. At build the decay across the loop is evened out so its end meets its start. 100 ms loops
  repeated the hiss audibly and beat against the note (heard on a Stage 4).
- Detect searches on each attack as the build cleans it, so the points it finds are those that
  play. On the interface take it loops the same 66 hits as the prototype and fades the same 9, the
  loops the same length to within 21 ms, starting a median 40 ms later, all matching 0.85 or better.
  Searched on the take with only its hum removed, three treble hits had swapped loop and fade.
- At build, the last 30 ms before the loop end are crossfaded (raised cosine) into the audio before
  the loop start, so the wrap is continuous. The crossfade length is the one build setting in the
  first version; a loop's length is set by dragging it. The match score is reported, and the hit grid
  warns below 0.5.
- The build shapes loops and fades from the marks alone, so a loop or fade moved in an editor is
  shaped the same way. It does so only for a take's regions: a directory of WAVs with their own loops
  is built as it is today.
- Written as a `smpl` loop, type 0, end frame inclusive, tied to the attack's region by its cue ID.

### Noise

There is no broadband denoiser in the first version. A spectral gate (STFT, profile from the take's
quiet frames) worked on the phone take but spoiled the treble loops of an interface take: a loud C5
loop's match fell from 0.86 to 0.15 at −12 dB. The best results of both takes used no gate (heard on a
Stage 4). Instead:

- **Too-quiet hits** are flagged and left out (see Hits). Treble boost and Dyno expose whatever noise
  a soft hit carries.
- **Hum.** Mains hum is tracked from the take, its frequency measured every 4 s. Each stroke gets a
  4th-order high-pass at 0.7 × its fundamental. Below F4 the strongest hum lines are also notched,
  but only where a line is at least 30 notch-widths from a partial of the note. The filters run
  forward only: run both ways, the notches rang ahead of the attack and, near a partial, on with the
  note (heard on a Stage 4). On the interface take, 93% of the floor was 50 Hz hum.
- **Hiss follow.** A low-pass that follows the note's own top end down as it dies. Its cutoff is the
  top of the highest 1/6-octave band where the note is still 6 dB over the take's hiss. It never
  rises again after the first 50 ms, so the strike keeps all its treble, and never falls under 1.5 ×
  the fundamental. It removes the hiss above the note and leaves the note alone where it is above the
  hiss. Treble boost and Dyno had made that hiss very loud (heard on a Stage 4). The take's hiss is
  measured in its quietest stretch with its hum out, on a 2048-point transform: 59 ms at 35,002 Hz,
  where the prototype's 46 ms was 2048 points at 44.1 kHz.
- A stroke is cleaned by its root's equal-tempered fundamental, so the build needs only the marks.
  Its filters run in over the 2.5 s of take before it.
- Takes should open with 3 to 5 s of silence, be free of ground loops, peak around −6 dBFS, and hold
  each note until it has died. The docs say so.

### Output

- 16-bit at 35,002 Hz, at build, mono or stereo as the take is. Brought to one peak first, the
  regions are dithered: rounded after adding triangular noise of one step either way, so what
  rounding loses becomes a steady hiss under the note, not distortion that follows it down.
  Silence stays silent, so a note faded out loops on nothing. Each stroke has noise of its own,
  so a chord's strokes don't share it and add up as little as possible. On the interface take,
  the dither leaves the last half second of every attack within 3 dB of the plain rounding
  (median 0 dB). It costs bits: the library grew from 9.1 to 11.4 MB.

### Where the code goes

- A new crate, `npno-prep`, of pure functions over sample buffers: no UI and no
  instrument. It builds for wasm, so the web app gets it too.
- All WAV handling moves into the new crate from `nord-format`: PCM reading (16-bit, 24-bit, float)
  and writing, the resampler, and the `cue `, `LIST`/`adtl` and `smpl` chunks, so WAV metadata has
  one home. Writing replaces only those chunks and copies the rest. `nord-format` stays a Nord format
  crate.
- The build accepts a region of a WAV as a stroke, named by the region's label.
- Pitch detection, hum tracking and hiss follow need an FFT. The crate has its own radix-2 real FFT,
  about 100 lines, so it adds no dependency or license to vendor. Everything else is plain arithmetic
  and IIR filters.
- `nord-cli` gets `nord piano prep TAKE.wav… --notes … [--layers N]`. It writes the regions and loops
  into the take (or a copy, or one per take into a directory), and prints a listing of hits, layers,
  loops and warnings. `nord piano build` takes one or more takes.
- The prep command is how the steps are checked on real takes before the editor exists. A user of the
  editor never needs it.
- `drawbar` gets the take editor over a WAV document, playing regions through the existing player.

```rust
pub struct Hit { pub number: usize, pub onset: usize, pub next: usize, pub level_db: f64, pub pitch: Option<f64> }
pub struct Cuts { pub attack: Range<usize>, pub keyup: Option<usize>, pub release: Option<Range<usize>>, .. }
pub enum Sustain { Loop { start: usize, len: usize, score: f64 }, Dies { fade: usize, end: usize } }

pub fn resample::resample(take: &Audio, rate: u32) -> Result<Audio, Error>;
pub fn plan::plan(mono: &[f64], rate: u32, options: &PlanOptions) -> Plan;  // hits, notes, layers, cuts, sustain
pub fn plan::Plan::regions(&self) -> Vec<Region>;
pub fn plan::Plan::loops(&self) -> Vec<Loop>;
pub fn sustain::even_loop(frames: &mut [f32], channels: usize, rate: u32, l: &Loop, o: &SustainOptions);
pub fn sustain::fade_out(frames: &[f32], channels: usize, rate: u32, fade: usize, o: &SustainOptions) -> (Vec<f32>, Loop);
pub fn clean::Noise::of(mono: &[f64], rate: u32) -> Noise;  // the take's hum and hiss
pub fn clean::Noise::stroke(&self, take: &[f32], channels: usize, start: usize, frames: usize, f0: f64) -> Vec<f32>;
```

### Tests

Per CONTRIBUTING, these test behavior rather than snapshots:

- Synthetic takes with known answers: decaying tones at chosen times, levels and pitches, with
  clicks, hiss, hum and gaps. Hit times, note assignment, layer order and cut points are asserted to
  within stated tolerances. The prototype has this generator; its 18-hit takes come out right on hits,
  notes and layers.
- A loop on a pure tone matches to within a stated correlation, and the crossfaded wrap has no step
  larger than a stated bound.
- The hum filter lowers a synthetic hum line by a stated amount and changes a tone 30 notch-widths
  away by less than a stated amount.
- A short self-recorded take as a fixture. It is ours, so it may be committed. The test checks the hit
  count and the note order the note list states.
- Region round trip: regions, labels and loops written to a WAV read back unchanged, every other
  chunk and the audio are byte for byte the same, and the build makes the same strokes from regions
  as from the same audio cut into separate WAVs.
- 24-bit and float takes read to the same samples as a 16-bit copy, within its precision.
- The Python prototype's output on the same fixture is a hand-checked reference while porting. It is
  not a committed oracle.

### Pull requests

The first four are written, on `mikesprad:feat/npno-prep`, and not yet opened.

- [ ] `npno-prep`: WAV handling moved from `nord-format`, with 24-bit and float reading, the
  resampler, and `cue `/`adtl` regions saved without touching other chunks. The build takes a
  labelled region as a stroke.
- [ ] Detectors: hits, notes and layers, cuts. `nord piano prep` writes regions. Fixtures and docs.
  Takes of one layer each. Releases keep their recorded balance in take builds.
- [ ] Sustain: loop search written as `smpl` loops on the regions (#257), and dying notes as fade
  regions, shaped at build.
- [ ] Hum removal and hiss follow as build rules. Loop search and release levels then run on the
  cleaned strokes.
- [ ] drawbar: the take editor over a WAV document, with Resample, Detect, playback, loop-wrap
  audition and region edits. Docs.

## Drawbacks

- It is a sizable addition: a new crate, a CLI command and a take editor.
- The detectors are heuristics tuned on few recordings: two takes of one Wurlitzer, a four-note
  pilot and synthetic takes. Other instruments and rooms will find cases they miss, so the hit grid's
  region edits are not optional.
- Without a broadband denoiser, a noisy recording gives a noisy library. Hiss inside the note's own
  band in its first second stays. The editor can only flag the worst hits and say what the recording
  needs.
- The plan depends on other editors keeping `cue `/`adtl` regions and `smpl` loops when they save. One
  that drops them loses the user's work on that take.

## Rationale and alternatives

| Option | Why not |
|---|---|
| Keep preparing strokes in outside editors | Slow (one file per stroke) and error-prone; no general editor knows strike, key-up, release or loop rules |
| Nord Sample Editor as the front end | It has key zones but no velocity layers, and it does not export WAVs |
| A take document (a plan over the source WAVs, with a stored sheet and overrides) | A new file format that duplicates what WAV and npno files already hold, and its overrides must be re-matched whenever a detector changes |
| Write regions on every edit | Rewrites a large WAV on each drag and floods sync and undo history; the other editors write on Save |
| Find loops on the take and refine them after the build's resample | Two loop searches, and the points the user dragged are not the points that play |
| A spectral gate on the take | Spoiled the treble loops of an interface take, and the best results came without it |
| Ship the Python prototype alongside drawbar | A second runtime and language; it can't run in the web app, and its UI would be separate |
| A batch tool only, with no UI | Hit detection will sometimes be wrong; fixing it needs listening and dragging, which is a UI |
| Record inside drawbar | Players already have recording software and interfaces; it adds device and latency work for little gain |

If nothing is done, library building stays limited to people willing to hand-edit every stroke.

## Prior art

- **Vendor libraries** (inferred from specimens): roots every 1 to 9 semitones, 1 to 15 attack layers
  per root, attacks up to about 6.5 s with a sustain loop near the end (m0/m1 = loop start and length),
  releases of 0.5 to 0.9 s with no loop, and every layer stored near the same peak.
- **drawbar today**: New ▸ Piano library builds from one WAV per stroke (#165), reads `smpl` loops
  (#256), fades rule-built strokes at note-off (#258) and loops by a per-kind curve (#264). #257 asks
  for an automatic loop search and #263 for tunable settings; this RFC covers the first and feeds the
  second. #279 levels and sets release trims from the audio.
- **The prototype**: `prep.py` in our project (Python, numpy/scipy), driven by a TOML sheet. Built
  through #279 as format 6 and loaded on a Stage 4, its Wurlitzer library from the interface take
  plays with working loops, dying treble notes, releases on key-up and little hiss (heard on a
  Stage 4).

## Unresolved questions

- **Right sidebar and the global cards.** The take editor's cards sit above the global ones while it
  has focus (see the walkthrough). This touches every editor, so it is for drawbar's other editors
  to follow or not.
- **Other editors.** Which editors keep `cue `/`adtl` regions and `smpl` loops when they save? To be
  checked before relying on them.
- **Default layer values, trims and ladders for v6** are the build's job, tracked in #278 and #279.
  The prototype's values: 4 layers at 0/22/50/85, 3 at 0/20/70.
- **The crate name.** Proposed: `npno-prep`, as it prepares the strokes for a `.npno` build. Open to a
  name that fits the other crates better.

## Future possibilities

- Linking a built npno back to its takes (#271 metadata, with #226), so a library can be rebuilt from
  them. Left out of the first version: it has many edge cases to test.
- More build settings (#263): levels, fades, loop length, the hum and hiss rules.
- Typed values for the cut and loop points, beside dragging.
- Stereo takes, with one loop shared by both channels.
- Other take shapes: one take per range of the keyboard, or a retake of a few notes merged into a
  library. Each needs the detectors to place gain changes and noise floors they can't yet tell
  apart.
- Writing each hit's measured pitch into the fine-tune table.
- A denoiser for the hiss inside the note's band, stronger along each note: the attack keeps its
  character with a light touch, while the tail is a few clean partials and can take a heavy one, or a
  mask around the note's harmonics.
- Other sound profiles. The first version is designed and tested on pianos: a sharp attack, a
  decaying tail and a release at key-up. The same steps need testing on sources with a different
  attack, sustain or decay, such as a slow swell or a steady sustained tone with no decay. Those may
  need other defaults, or generic templates for the build to apply.
