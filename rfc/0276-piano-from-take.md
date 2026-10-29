# RFC-0276: Build piano libraries from a recorded take

- Start date: 2026-10-28
- Authors: mikesprad
- RFC PR: [jmoo/drawbar#276](https://github.com/jmoo/drawbar/pull/276)
- Tracking issue: none yet

## Summary

Turn one long recording of an instrument into the strokes of a piano library, with the recording
itself as the document. Detectors find the hits, name their notes and layers, mark each attack and
release, and find a sustain loop. They write the results into the WAV as standard regions and loops,
labelled with stroke names. New ▸ Piano library then builds from "a WAV, or a named region of one".
Levelling, fades and the loop crossfade are build rules. Background noise removal writes a new WAV.
The processing goes into a new crate of pure functions over sample buffers, which also takes over all
WAV reading and writing. It is first exposed as a `nord-cli` command and then as a drawbar UI. A
Python prototype already does all of these steps; libraries built from its output play on a Stage 4.

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

The prototype shows the gap can be closed: a Wurlitzer recording made on a phone (13 roots,
3 layers, releases and loops) went from one WAV to a library that plays on a Stage 4 in a single
run. Hit splitting, note naming, layer order, cuts and loops all came out right without hand
editing; a sheet listing the notes and a few skipped hits was the only input.

## Guide-level explanation

Recording stays outside drawbar. Anyone recording an instrument already has an audio interface and
software that records, so drawbar starts from the WAV.

1. **Open a take** (one or more WAVs). Several takes are normal: one per octave, or a retake of a few
   notes.
2. **Find strokes.** For each take, the notes in playing order: a list (`E3 G3 A#3 …`), or a start
   note, a step in semitones and a count. Then the number of layers, and how they are chosen. These
   are inputs to the detectors, not stored; the detectors write regions and are done:
   - **Playing order**: the first hit of a note is the loudest layer. This suits players who play
     loud, medium, soft, and recordings whose level can't be trusted (a phone's automatic gain made
     measured loudness useless on our test take).
   - **Loudness**: hits sorted by measured level, with near-duplicates within 2 dB dropped and the
     kept layers spread evenly in dB.
3. **Noise.** Off, Light (−6 dB), Medium (−12 dB) or Strong (−20 dB), with before/after playback of a
   gap and a note. The profile comes from the take's opening silence and the gaps between hits. It
   writes a new WAV next to the take.
4. **Hit grid.** One row per note, one column per hit. Each cell plays its region and shows its
   layer, its length, whether a release region was found, and the loop match. Warnings mark a pitch
   that disagrees with the note list, a hit with no key-up, and a weak loop match. Each edit changes
   the WAV's regions:
   - skip a hit (a stray noise, a stopped note): remove its regions;
   - set its layer, swap it with another hit, or give it a different note: relabel the region;
   - drag its start, key-up, release end and loop on the waveform: move the region or loop.
5. **Build.** New ▸ Piano library takes the WAV and builds a stroke from each labelled region, so
   template, kind, gain and format work as they do now.

The WAV is the record. A user decides only where each stroke sits, what it is, and where its loop
is, and standard RIFF chunks hold all three:

- a `cue ` point and a `LIST`/`adtl` `ltxt` region per stroke, labelled with the name the build
  already reads (`060-b0-l00`, `060-b2-l00`);
- a `smpl` loop tied to its region's cue ID.

Nothing else is stored: no sheet, and no overrides to re-match when a detector changes. Other
editors can read and write the same regions. The link from a built npno back to its takes can be
#271 metadata, so a library can be rebuilt from them.

| Step | Today | With this RFC |
|---|---|---|
| Split a long take into hits | — | Onset detection that skips clicks and handling noise |
| Name each hit's root | `060-b0-l00.wav` names or the dialog | From a note list in playing order, checked against detected pitch |
| Choose velocity layers | `lNN` or `vNN` names | By playing order or measured loudness, with per-hit override |
| Cut the attack | Whole WAV | Strike detection, key-up detection, length cap |
| Cut the release | Separate WAV by hand | From the key-up of the same hit |
| Level | Per-WAV gain (#165) | Build rule: each stroke to one peak; releases to their own peak |
| Loop | From a WAV's `smpl` loop (#256) | Loop search, written as a `smpl` loop on the region; crossfade at build; covers #257 |
| Background noise | — | Spectral gate learned from the take's silence, with a strength setting; writes a new WAV |
| Layer values, thresholds, trims, build | New ▸ Piano library (#267) | Builds from labelled regions as well as whole WAVs; v6 defaults in #278 |

## Reference-level explanation

Provenance uses the forms in `CONTRIBUTING.md`. "Heard on a Stage 4" means a by-ear check on our
instrument (Stage 4, OS 1.64), loading the library through Sound Manager.

### Hits

- Envelope: RMS in 10 ms frames, after a 15 Hz highpass.
- Onset: the envelope rises 15 dB above its minimum over the previous 150 ms and is still 15 dB above
  the noise floor (the 10th-percentile level) 100 ms later. The second test rejects clicks. Of onsets
  closer than 1 s, the loudest wins.
- Level of a hit: the envelope's maximum over its first 100 ms.
- Pitch of a hit: harmonic product spectrum over 50 to 550 ms after the onset.

### Notes and layers

- Hits are assigned by walking the note list in order. A hit moves on to the next note when its pitch
  class is nearer that note's than the current one's. Skipped hits take no part. Comparing pitch
  class tolerates the octave errors common on low notes.
- A hit more than half a semitone from its note's pitch class gets a warning.
- More hits than layers: loudness mode keeps the hits nearest evenly spaced levels between the
  loudest and softest; playing-order mode keeps the first N.
- The number of layers can differ per range of roots, for example 4 layers over the middle octaves
  and 3 elsewhere.
- Layer values are named into the stroke names, and the build turns them into thresholds as it does
  now. See Unresolved questions for the default spread.

### Cuts

- Strike: the first sample within −12 dB of the hit's early peak, minus a 1 ms pre-roll. Vendor
  attacks reach −6 dB within 1 ms (inferred from specimens). Key noise kept before the strike delayed
  the attack, more so on keys below the root (heard on a Stage 4).
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
- Release: from 30 ms before key-up, up to 0.7 s by default, with a 5 ms fade in and a fade over its
  last fifth. Vendor releases run 0.5 to 0.9 s (inferred from specimens).
- No fade at the end of a looped attack: the end sits inside the loop. A fade there produced a loud
  square wave or fast clicking once the loop played (heard on a Stage 4).

### Level

- Every attack is scaled to one peak (−13 dBFS by default), whatever its layer. Vendor libraries keep
  every layer near the same peak (inferred from specimens: attack layers peak at −12.5 to −17.5 dBFS),
  and the playback gain does the rest.
- Releases are scaled to their own peak. Scaled with their attack's gain, they sat 35 to 57 dB down
  and could not be heard (heard on a Stage 4).

### Loop

- The loop ends 150 ms before the end of the attack, clear of the damper at key-up.
- Its length starts at a whole number of the note's periods near 100 ms. It is then tuned within one
  period either side for the best normalized correlation between the audio before the loop end and
  the audio before the loop start.
- At build, the last 30 ms before the loop end are crossfaded (raised cosine) into the audio before
  the loop start, so the wrap is continuous. The match score is reported, and the hit grid warns below 0.5.
- Written as a `smpl` loop, type 0, end frame inclusive, tied to the stroke's region by its cue ID.
  In the prototype, one loop per stroke WAV at 35,002 Hz, which #256 reads; loops built this way
  sustain until key-up without clicks (heard on a Stage 4). In a take at another rate the build's
  resample moves the loop points; see Unresolved questions.

### Noise

- Quiet frames: 50 ms frames within 4 dB of the take's 10th-percentile level.
- Spectral gate: STFT with a 4096-sample window. Each bin's noise profile is the 90th percentile of
  the quiet frames, and the gain is `sqrt(1 − 1.5 · profile / power)`, floored at the chosen strength.
  Note content well above the profile passes unchanged.
- The 90th percentile, not the mean, because the noise level wandered between gaps in the test take.
- Hum is gated, not notched. Narrow notches rang for about 0.5 s when a note sat on a hum line (F4 at
  349 Hz against 350 Hz), and they raised the gaps by 4 dB.
- On the phone take, the −20 dB setting took the gap before the first note from −51 to −61 dB. On the
  built library, hiss was inaudible on normal playing (heard on a Stage 4). Long held chords still hiss,
  worst with bright timbres (heard on a Stage 4).
- Takes should open with 3 to 5 s of silence. The docs say so.

### Output

- 16-bit mono at 35,002 Hz with TPDF dither, from a band-limited resample.
- Stereo is not in the first version: the prototype mixes to mono. A stereo version needs one loop for
  both channels.

### Where the code goes

- A new crate, `nord-prep` (name open), of pure functions over sample buffers: no UI and no
  instrument. It builds for wasm, so the web app gets it too.
- All WAV handling moves into the new crate from `nord-format`: PCM reading and writing, and the
  `cue `, `LIST`/`adtl` and `smpl` chunks, so WAV metadata has one home. `nord-format` stays a Nord
  format crate.
- The build accepts a region of a WAV as a stroke, named by the region's label.
- The STFT needs an FFT. `rustfft` (MIT/Apache, pure Rust, builds for wasm) is the candidate.
  Everything else is plain arithmetic.
- `nord-cli` gets `nord piano prep TAKE.wav --notes … --layers N`. It writes the regions and loops
  into the take (or a copy), and prints a listing of hits, layers and warnings.
- `drawbar` gets the hit grid over a take WAV, playing regions through the existing player.

```rust
pub struct Take { pub rate: u32, pub samples: Vec<f32> }
pub struct Hit { pub onset: usize, pub next: usize, pub level_db: f32, pub pitch_hz: Option<f32> }
pub struct Cuts { pub attack: Range<usize>, pub release: Option<Range<usize>> }
pub struct LoopPoints { pub start: usize, pub len: usize, pub match_score: f32 }

pub fn hits(take: &Take) -> Vec<Hit>;
pub fn assign(hits: &[Hit], notes: &NoteList, layers: &LayerRule) -> Vec<Assigned>;
pub fn cut(take: &Take, hit: &Hit, options: &CutOptions) -> Cuts;
pub fn find_loop(attack: &[f32], rate: u32, pitch_hz: f32, keyup: usize) -> Option<LoopPoints>;
pub fn denoise(take: &Take, strength: Strength) -> Take;
```

### Tests

Per CONTRIBUTING, these test behavior rather than snapshots:

- Synthetic takes with known answers: decaying tones at chosen times, levels and pitches, with
  clicks, hiss, hum and gaps. Hit times, note assignment, layer order and cut points are asserted to
  within stated tolerances. The prototype has this generator; its 18-hit takes come out right on hits,
  notes and layers.
- A loop on a pure tone matches to within a stated correlation, and the crossfaded wrap has no step
  larger than a stated bound.
- The gate lowers a gap by at least the chosen strength, and changes a tone well above the profile by
  less than a stated amount.
- A short self-recorded take as a fixture. It is ours, so it may be committed. The test checks the hit
  count and the note order the note list states.
- Region round trip: regions, labels and loops written to a WAV read back unchanged, and the build
  makes the same strokes from regions as from the same audio cut into separate WAVs.
- The Python prototype's output on the same fixture is a hand-checked reference while porting. It is
  not a committed oracle.

### Pull requests

- [ ] `nord-prep`: WAV handling moved from `nord-format`, with `cue `/`adtl` regions. The build
  takes a labelled region as a stroke.
- [ ] Detectors: hits, notes and layers, cuts. `nord piano prep` writes regions. Fixtures and docs.
- [ ] Loop search, written as `smpl` loops on the regions (#257).
- [ ] Noise gate with strength, writing a new WAV.
- [ ] drawbar: hit grid over a take, with playback and region edits. Docs.

## Drawbacks

- It is a sizable addition: a new crate, an FFT dependency, a CLI command and a hit grid UI.
- The detectors are heuristics tuned on few recordings: one phone take of a Wurlitzer, a four-note
  pilot and synthetic takes. Other instruments and rooms will find cases they miss, so the hit grid's
  region edits are not optional.
- Denoising always costs some of the sound. A strength setting moves the trade-off to the user rather
  than removing it.
- The plan depends on other editors keeping `cue `/`adtl` regions and `smpl` loops when they save. One
  that drops them loses the user's work on that take.

## Rationale and alternatives

| Option | Why not |
|---|---|
| Keep preparing strokes in outside editors | Slow (one file per stroke) and error-prone; no general editor knows strike, key-up, release or loop rules |
| Nord Sample Editor as the front end | It has key zones but no velocity layers, and it does not export WAVs |
| A take document (a plan over the source WAVs, with a stored sheet and overrides) | A new file format that duplicates what WAV and npno files already hold, and its overrides must be re-matched whenever a detector changes |
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
  second.
- **The prototype**: `prep.py` in our project (Python, numpy/scipy), driven by a TOML sheet. Built
  through #267 and loaded on a Stage 4, its Wurlitzer library plays with working loops, releases on
  key-up and little hiss after denoising. Some hiss remains on long held tails, worst with bright
  timbres.

## Unresolved questions

- **Loop points and the sample rate.** The loops are tuned at 35,002 Hz so the build doesn't move them.
  In a 48 kHz take the build's resample shifts them. Either the build refines and crossfades the loop
  after resampling, or the take is resampled to 35,002 Hz first, as a new WAV like denoise.
- **Build rules and their settings.** Levelling (one attack peak, a separate release peak), the loop
  crossfade and the pre-roll become build rules, so their settings sit with #263.
- **Other editors.** Which editors keep `cue `/`adtl` regions and `smpl` loops when they save? To be
  checked before relying on them.
- **Default layer values, trims and ladders for v6** are the build's job, tracked in #278.
- **The crate name.**

## Future possibilities

- Stereo takes, with one loop shared by both channels.
- Several takes merged into one library, such as a retake of a few notes.
- Writing each hit's measured pitch into the fine-tune table.
- Denoising that grows stronger along each note. The attack keeps its character with a light gate,
  while the tail is a few clean partials and can take a heavy one, a falling low-pass, or a mask
  around the note's harmonics. That would remove the hiss that remains on long held tails.
- A pass for regular clicks: a tick at a fixed period, fitted to a timing grid. One test recording
  needed it.
- Other sound profiles. The first version is designed and tested on pianos: a sharp attack, a
  decaying tail and a release at key-up. The same steps need testing on sources with a different
  attack, sustain or decay, such as a slow swell or a steady sustained tone with no decay. Those may
  need other defaults, or generic templates for the build to apply.
