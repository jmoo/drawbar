# What is supported

drawbar was developed against a Nord Electro 5, and that is where its claims have
been tested. Files from other instruments are supported from documentation and
sample files. Connecting other instruments over USB has not been tried.

## Files

| Files | What drawbar does | Tested on an instrument |
|---|---|---|
| Electro 5 programs, live slots, set lists and settings | View, edit, transfer | Yes |
| Stage 2, 3 and 4 programs and live slots, and Stage 3 and 4 presets | View, edit | No |
| Sample instruments (`.nsmp`, `.nsmp3`, `.nsmp4`) | Decode, edit, encode, audition, transfer | Playback of v2 files. Files encoded as v3 or v4 have not been played |
| Piano libraries (`.npno`) | Decode, trim, split, rename, retune, remap, build from WAVs with loops, transfer | Trimmed, built and re-encoded libraries play, mono and stereo, across every key they cover. A library built without a template sounds the same as one built with one while a key is held, and fades a note out within about 30 ms of key-up. Loops built from WAVs sustain on an Electro 5 until the key is released, mono and stereo, on their own keys and transposed ones, and fade at the rate their kind and `--loop-fade` set. Loops built by hand in the same layout sustain on a Stage 4. Libraries are written in format 6.x unless 5.x is chosen. An Electro 5 on OS 2.04 plays both formats, built by rules or against a 6.x template, with their velocity layers, trims, releases and loops. A release plays at the level the note has fallen to at key-up. The velocities drawbar gives for each layer are an Electro 5's. Renames, retunes, remaps and a narrowed key range have not been played |
| Electro 5 bundles (`.ne5pbundle`, `.ne5tbundle`) | Import to a folder, export from this computer or the instrument. Only uncompressed bundles open, as Nord Sound Manager writes them | Nord Sound Manager opens the bundles drawbar writes and sends them to the instrument. Bundles made from the instrument match what nord-cli makes. A piano or sample built in drawbar has not yet gone to the instrument in a bundle |
| Other Nord files | Recognized and kept byte for byte, without editing | |
| Text files | Edit, keep beside your files | Never sent to an instrument |

## USB

Reading and writing programs, set lists, live slots, settings, samples and pianos
works on the Electro 5 from macOS, Linux and the browser. Windows builds pass the
protocol tests, and drawbar on Windows has connected to a Stage 4 and listed its
storage, but nothing has been written from Windows. The Stage 4 is the only other
instrument connected so far, and no file has been read from it or written to it, so
USB support for other models cannot be guaranteed.

drawbar and nord-cli find programs, samples and the rest by the name the instrument
gives each kind of storage, so another model's folders show what it keeps in them.
The Stage 4's organ, piano and synth presets get folders of their own. A kind of
storage drawbar has no name for, such as the Stage 4's piano pedal sounds, is listed
under the instrument's own name and is read only. Writing to another model follows
what the Electro 5 does and has not been tried, so drawbar and nord-cli warn before
every such write.

The safeguards drawbar applies when it writes, listed under
[Safety](../drawbar/instrument.md#safety), have been confirmed on the Electro 5.

## The library in the browser

Every browser keeps a library in its own storage for drawbar.app. Safari deletes
it once Safari has been used for seven days without a visit to drawbar.app,
unless drawbar.app is added to the Home Screen. Other browsers may
clear it when space runs low, unless they have promised to keep it. **File ▸
Save a copy of the library** keeps a copy; see
[Your files](../drawbar/this-computer.md#keeping-a-copy-in-the-browser). Chrome and Edge
can also open a folder on your computer as the library, and Brave can once
`brave://flags/#file-system-access-api` is turned on. Outside Chrome, moving a
file in that folder copies it, which takes longer for a large one. Firefox and
Safari cannot open a folder. drawbar's tests run the library in Chromium and
Firefox on every change, in Safari when the library changes, and in Edge before a
release.

## MIDI controllers

The sample and piano key maps can be played from a MIDI controller. The desktop
app listens on every MIDI input the computer has. In the browser, Chrome, Edge
and Firefox can hear a controller. Safari cannot, so drawbar grays out the menu
item there. Firefox asks you to allow MIDI access for the site first.

## What "tested" means

Every file drawbar writes is read back and checked to be identical, byte for
byte. That proves the bytes survive. It does not prove that every value means
what drawbar says, or that every new sound plays as intended. Where a claim has
been confirmed by playing it on an instrument, this page says so.

Developers can read how the checks work in [Testing](../reference/testing.md)
and [File formats](../reference/file-formats.md).
