# What is supported

The Nord Electro 5 is the instrument used to develop and test drawbar.
You can also edit files from several Stage models on your computer. Use the
tables below to check your file, your USB connection and your browser separately.

## Files

| What you have | What you can do |
|---|---|
| Electro 5 programs, live slots, songs and settings | View, edit and transfer. Tested on the Electro 5. |
| Stage 2, 3 and 4 programs and live slots, and Stage 3 and 4 presets | View and edit. Support comes from documentation and specimen files. These edits have yet to be tested on the instruments. |
| Sample instruments (`.nsmp`, `.nsmp3`, `.nsmp4`) | Listen, edit zones, extract WAVs and transfer. The app builds v2 files from WAVs or Sample Editor projects. V2 playback has been tested. Files encoded as v3 or v4 have not been played on an instrument. |
| Piano libraries (`.npno`) | Listen, trim, rename, tune, map keys and build from WAVs, including loops. The CLI can also split and re-encode libraries. [Hardware playback results](#piano-libraries) |
| Electro 5 program and song bundles (`.ne5pbundle`, `.ne5tbundle`) | Import and export uncompressed bundles in the form Nord Sound Manager writes. Sound Manager opens drawbar's bundles and sends them to the instrument. Bundles from the instrument match those made by nord-cli. Sending a drawbar-built piano or sample *inside* a bundle remains untested. |
| Other recognized Nord files | Keep and copy the original file byte for byte. Editing depends on the format. |

WAV recordings, Sample Editor projects and text notes can live alongside your
sounds. Build a project into a sample instrument before sending it to a slot.
Notes stay on the computer.

## USB

| Instrument | Confirmed on hardware |
|---|---|
| Electro 5 | Reading and writing programs, set lists, live slots, settings, samples and pianos from macOS, Linux and the browser. Moving, deleting, renaming, duplicating and selecting slots from macOS. The [write safeguards](../drawbar/instrument.md#safety) have also been tested here. |
| Electro 7 | Connected and listed storage from Windows. File reads and writes on this model remain untested. |
| Stage 4 | Connected and listed storage from Windows. File reads and writes on this model remain untested. |
| Other models | No USB connection has been tested. |

Windows builds pass the protocol tests. Writing from Windows has yet to be
tried on hardware. Writing to a model other than the Electro 5 follows what the
Electro 5 does and has not been tried, so the app and the CLI warn before every
such write.

Another model's folders show what it keeps in them: the Stage 4's organ, piano
and synth presets get folders of their own. A storage folder drawbar doesn't
recognize, such as the Stage 4's piano pedal sounds, appears under the
instrument's own name and is read-only.

## The library in the browser

All of these recent desktop browsers can open files and keep a library for
drawbar.app. Choose a browser by what else you want to connect.

| Browser | Nord over USB | Folder on disk | MIDI input |
|---|---|---|---|
| Chrome or Edge | Yes | Yes | Yes |
| Brave | Yes | With setting enabled | Yes |
| Firefox | No | No | Yes, with permission |
| Safari | No | No | No |

To open a folder in Brave, enable `brave://flags/#file-system-access-api`.
Outside Chrome, moving a file in that folder copies it, which takes longer for
a large one.

In Safari, seven days of Safari use without a visit to drawbar.app can clear
the site's library, unless drawbar.app is added to the Home Screen. The Home
Screen app has storage of its own. Other browsers may clear site data when space is low unless they have
granted persistent storage. [Back up the library or move it to another browser](../drawbar/backup.md#keeping-a-copy-in-the-browser).

## MIDI controllers

Listening for MIDI lets you play the sample or piano open in drawbar through
the computer. The desktop app listens on all available MIDI inputs. In a
browser, the table above shows where the option is available.
[Set up a controller](../drawbar/overview.md#midi-controllers).

## Piano libraries

On an Electro 5 running OS 2.04, both 5.x and 6.x piano libraries play, built
from drawbar's rules or with a 6.x template. Trimmed, newly built and
re-encoded libraries have played in mono and stereo across their covered keys.

- **Touch.** The displayed velocity ranges are for the Electro 5. A 6.x library
  built without a template switched layers within two velocity steps of the
  predicted values. Layer levels followed their trims.
- **Sustain.** Mono and stereo WAV loops sustained at their original and
  transposed pitches on the Electro 5. Held notes and loops faded at the rates
  set by the library kind and loop fade. Hand-built loops of the same layout
  also sustained on a Stage 4.
- **Release.** A note built without a template faded within about 30 ms of
  key-up. Release playback followed the note's level at key-up, about 35 dB
  below it in the 6.x test without a template.

Renamed or retuned pianos, remapped keys and narrowed key ranges have yet to
be played on hardware. [Trim or build a piano](../drawbar/pianos.md).

## What "tested" means

File tests check whether reading and writing preserves the original bytes.
Listening tests answer a different question: how the keyboard actually plays
those bytes. The hardware results above describe what has been tried.
[How the project tests files and instruments](../reference/testing.md).
