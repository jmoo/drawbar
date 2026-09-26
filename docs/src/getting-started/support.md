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
| Piano libraries (`.npno`) | Decode, trim, split, rename, retune, remap, build from WAVs, transfer | Trimmed, built and re-encoded libraries play, mono and stereo, across every key they cover. A library built without a template sounds the same as one built with one. Renames, retunes, remaps and a narrowed key range have not been played |
| Other Nord files | Recognized and kept byte for byte, without editing | |
| Text files | Edit, keep beside your files | Never sent to an instrument |

## USB

Reading and writing programs, set lists, live slots, settings, samples and pianos
works on the Electro 5 from macOS, Linux and the browser. Windows builds pass the
protocol tests but have not been run against an instrument. No other instrument
has been connected, so USB support for other models cannot be guaranteed.

The safeguards drawbar applies when it writes, listed under
[Safety](../drawbar/instrument.md#safety), have been confirmed on the Electro 5.

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
