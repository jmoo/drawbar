# nord-cli

A command-line tool (`nord`) over [`nord-format`](../nord-format) and
[`nord-usb`](../nord-usb). Its job is to **dogfood the libraries** — read Nord
files, check they round-trip, and drive an attached instrument — not to be a
product (this will change once the libraries have stabilized). 
Anything the libraries can do should be reachable from here, which is
how API friction gets found.

Expect large changes to this tool over time while I develop the libraries.

> [!CAUTION]
> # ⚠️ 🧱 USE AT YOUR OWN RISK — THIS TOOL CAN BRICK YOUR DEVICE 🧱 ⚠️
>
> Until this project reaches 1.0, **do not use this tool to speak to a real Nord
> device** unless you are using it to actively develop these libraries.
>
> **I am not responsible for what you do with this tool.**

## Commands

| | |
|---|---|
| `inspect` | Decode file(s) and print a readable summary |
| `verify` | Re-encode file(s) and check the bytes come back identical |
| `device` | The instrument itself — what is on the bus, and what it holds |
| `program` | Programs on the instrument (object class 4) |
| `setlist` | Set lists on the instrument (object class 5) |
| `live` | The three Live slots (object class 6) |
| `settings` | The global settings singleton (object class 7) |
| `sample` | Sample instruments — the library (object class 3), or `.nsmp` files |
| `raw` | Hidden: the same verbs, addressed by class number |

`inspect` and `verify` work on files. The other nouns are the protocol's object
classes, and normally talk to an attached instrument — but the read-only verbs
(`get`, `info`, `deps`) and `edit` also take a file in place of a slot. `program`
and `setlist` share one verb vocabulary:

```
get put            transfer
move rename duplicate delete select   organisation
info deps          interrogation
edit               content (program, live, settings, sample)
```

`live` keeps only the read-only subset plus `edit` — the live buffer is the panel
as it stands, so there is nothing to name, delete, or select. `settings` is a
singleton with nothing to organise at all, so `edit` is its whole verb set.

`nord raw --class N` is those same verbs with the class given as a number. It is
how to reach a class that has no noun of its own — pianos are class 1.

Slots are written **`BANK:SLOT`**, the way the instrument and Nord Sound Manager
show them — `7:4` is bank 7, slot 4, both counted from 1. (`7-4` also parses.)

### Output and interaction

- **Data on stdout, everything else on stderr.** `nord program get 7:4 | grep
  transpose` sees the summary and nothing else.
- **Colour and unicode only on a terminal.** `--color=auto|always|never`;
  `NO_COLOR` in the environment forces colour off. Piped output is plain ASCII.
- **A pipe is non-interactive.** On a terminal a destructive command asks for
  confirmation; off one, a missing `--yes` is an error rather than a prompt.

## Working with files

```sh
nord inspect patch.ne5p              # readable summary
nord inspect *.ne5p                  # several at once
nord inspect --raw song.ne5t         # full Debug dump
nord verify *.ne5p                   # round-trip check
```

`inspect` exits non-zero if any file fails to parse. For an Electro 5 program:

```
LA Grand.ne5p
  type:      Electro 5 program (ne5p)
  location:  bank 1 slot 5
  lower:     Piano  octave +0  sustain yes  control no
  upper:     Sample  octave +0  sustain yes  control no
  split:     no
  transpose: +1  (no)
  part mix:  49.6/50.0 (lower/upper %)
  gain:      119
  piano:     category 0  model 2  clav 0  acoustics 1  touch 2  mono no
  sample:    number 92  attack 14  decay/rel 87  dynamics 1  filter yes
  depends:   piano 0x3d4b3e14  sample 0x65d8c5a1
  fx:        stored value, with the panel's 0-10 reading where it applies
    fx1   off
    fx2   upper  chorus 1   rate 45  deep no
    fx3   off
    delay upper  feedback 2  tempo 24  wet 11 (0.9)  ping-pong no
    reverb stage      wet 23 (1.8)
    eq    lower        bass 74  freq 94  gain 70  treble 64
    rotary speed slow  stop off
```

`depends:` is the piano and sample the program references. Those ids are the same
values the instrument reports for that program over USB, so `nord program deps` on
the same slot will name them — which is the only way to resolve an id, since the
file itself stores no names.

For an organ program, both presets of all four models are shown, with the
selected model marked `*` and its active preset `<`:

```
  organ:     b3+bass selected (*), drawbar positions 0-8
   *b3    p1< 04.......  vib off  perc off
   *b3    p2  000000000  vib off  perc off
    vox   p1< 888800000  vib V3
    vox   p2  888800000  vib V3
   (* = selected model, < = its active preset)
```

Both presets are shown because in **b3+bass** the two are different instruments:
preset 1 is the bass manual, whose two drawbars live outside the nine-nibble
block. It renders as `04.......` rather than nine positions, since the nine
nibbles hold stale values in that mode.

Songs list their four program slots; settings print the decoded System, MIDI and
Sound menus plus the selection the instrument restores at power-up; bundles need
the `bundle` feature (enabled here) to open.

### The slot verbs on a file

`get`, `info` and `deps` take a file wherever they take a `BANK:SLOT`, so a file
already on disk can be read with no instrument attached:

```sh
nord program get patch.ne5p             # the same summary the slot form prints
nord program get patch.ne5p --body -o patch.body   # strip the CBIN header
nord program info patch.ne5p            # the header: format tag, version, crc32
nord program deps patch.ne5p            # stored library ids
```

A path that exists wins over a slot reading, so a file named `7:4` is still a
file. What a file does not carry is reported as living on the instrument rather
than guessed at: files store no slot name, and `deps` on a file prints ids only —
the slot form asks the instrument, which attaches the names.

### `verify`

Parses each file, writes it back, and checks the bytes are identical — reporting
the offset of the first difference if not. It is the only thing that exercises
the write path end to end:

```
$ nord verify song.ne5t settings.ne5s grand.npno
ok     song.ne5t (62 bytes)
ok     settings.ne5s (78 bytes)
DIFFER grand.npno (in 209564996 bytes, out 20; first difference at the end (length differs))
error: 1 of 3 file(s) did not round-trip
```

(A `DIFFER` on a piano is expected for now — only its header is parsed and
re-emitted; see the `nord-format` format table. Samples round-trip.)

## Working with an instrument

Close Nord Sound Manager first — it claims the vendor interface exclusively, and
`nord` cannot attach alongside it.

```sh
nord device status                      # inventory per class; --json for machines
nord device info                        # what is attached, from the USB descriptors

nord program get 7:4                    # summary to stdout
nord program get 7:4 -o patch.ne5p      # write the .ne5p instead
nord program put patch.ne5p 7:4 --yes
nord program move 8:13 7:16 --yes
nord program delete 7:50 7:49 --yes
nord program info 7:4                   # size, format, version, name, crc32
nord program deps 7:4                   # piano/sample dependencies, with names
nord program select 2:12                # load live on the instrument
nord program rename 6:13 "foo" --yes
nord program duplicate 7:2 7:3 --yes
```

The same verbs work on `nord setlist`, and on `nord raw` with an explicit class:

```sh
nord setlist get 1:1
nord raw --class 1 info 1:1             # a piano
nord raw --class 5 get 1:1 --body -o setlist.body
```

`--body` saves the wire body verbatim instead of wrapping it in a CBIN header.
Use it for classes whose header layout is not known, where the wrapped file would
look plausible and be wrong.

### Safety

Every mutating command **reads the slot first, says what it will touch, then
refuses without `--yes`**. Off a terminal, running it without the flag is a real
dry run:

```
$ nord program move 7:2 7:3
moving "Africa Split" from bank 7 slot 2 to bank 7 slot 3 — OVERWRITING "Squabble B"
error: refusing to proceed without --yes
```

`move` and `duplicate` name the *destination's* current occupant, not just the
source — that is the thing about to be lost.

Two other guards worth knowing about:

- An empty slot reports `bank 5 slot 42 is empty` rather than a raw status code.
- Every command closes its transaction **even when it fails**. An abandoned
  session leaves the instrument stuck on a progress screen with no way out but a
  power cycle.

`--yes` means *don't ask me*. There is no `--force`.

Writing into an occupied slot is a **delete followed by a write** — the
instrument refuses to overwrite in place. `nord` reads the occupant first and
puts it back if the write fails; if the restore fails too, the bytes are written
to a `nord-rescued-BANK-SLOT.ne5p` in the working directory.

## Editing an object

`edit` is the only verb that changes what is *inside* an object, and it exists on
four nouns: `nord program edit`, `nord live edit` (the live buffer is the
program body under another tag, so the fields are identical), `nord settings
edit` (`panel.*` for the menus, `selection.*` for the restored panel state), and
`nord sample edit` (below). For the first three the field paths are
`nord-format`'s own names, generated from the panel declarations, so `--fields`
lists whatever the library currently knows:

```sh
nord program edit --fields                       # what is settable, and what it takes
nord program edit patch.ne5p --set center_panel.gain=96 -o out.ne5p
nord program edit patch.ne5p --set effects_panel.fx1_rate=96 --yes     # in place
nord program edit 7:4 --set center_panel.split=true --dry-run
nord program edit --set center_panel.gain=64 -o blank.ne5p             # a fresh program
```

A file and a slot are the same command; the slot form is a read-modify-write over
USB, so it asks before writing. Editing a file in place asks too — pass `-o` to
write somewhere else instead.

For `live` and `settings` the slot form reads off the instrument but **refuses to
write back**: writing is a delete followed by a write, and whether the live
buffer or the settings singleton survives that is unconfirmed on hardware. An
edited slot of either class stops at a file, via `-o`.

**A value is spelled the way `nord inspect` and `--fields` print it**, and one
the field cannot hold is rejected before anything is written:

```
$ nord program edit patch.ne5p --set center_panel.gain=200 --dry-run
error: "200" is not a value of gain (accepts 0 .. 127)
```

A stored value the library does not recognise prints as `Unknown(9)`, so that is
also how it is written: `--set …=Unknown(9)`. A bare `9` matches nothing.

`--dry-run` reports the fields and the bytes that would change, and writes
nothing:

```
$ nord program edit patch.ne5p --set center_panel.transpose=-5 \
    --set center_panel.transpose_enabled=true --dry-run
center_panel.transpose_enabled           false -> true
center_panel.transpose                   0 -> -5
  byte 0x0018  0x01 -> 0xad  (body crc32)
  byte 0x0019  0xe8 -> 0x13  (body crc32)
  byte 0x001a  0x1d -> 0x5e  (body crc32)
  byte 0x001b  0xe7 -> 0x05  (body crc32)
  byte 0x0030  0x00 -> 0x01
  byte 0x0031  0x60 -> 0x10
```

> [!WARNING]
> **Some fields only mean something in pairs.** `center_panel.transpose` is
> ignored while `center_panel.transpose_enabled` is clear, the instrument never
> clears that bit once it is set, and an untouched program holds `+1` rather than
> `0`. Setting one half without the other warns; it is not refused.

### `nord sample edit`

A sample instrument is mostly encoded audio, so its settable fields are the ones
the format can patch in place without touching a sample: the name, and each
zone's root key and top note. Notes are spelled as names (`C4`, `F#3` — middle C
is C4) or numbers 0–127, and zones are numbered from 1, top of the keyboard
first, the way `inspect` lists them:

```sh
nord sample edit inst.nsmp --fields
nord sample edit inst.nsmp --set name="My Piano" --set zone2.top_note=C4 -o out.nsmp
nord sample edit inst.nsmp --set zone1.root_key=48 --dry-run
```

## Status

This is a mostly LLM generated tool to validate the mostly handwritten nord-format
crate. It is also used to test the nord-usb crate. Once nord-format and nord-usb
are stable, I will re-write this to be a nice easy to use cli for interacting with
your nord devices and files.

> [!CAUTION]
> ## 🚫 DO NOT USE THIS FOR NON-DEVELOPMENT PURPOSES 🚫

## Build & run

From the lab workspace:

```sh
nix develop
cd crates
cargo run -p nord-cli -- inspect patch.ne5p
```

Or as a Nix package — `nix build .#nord-cli` installs the `nord` binary.

## Disclaimer

Not affiliated with, authorized, or endorsed by Clavia DMI AB. "Nord", "Clavia",
and "Electro" are trademarks of Clavia DMI AB, used here only to identify the
hardware these formats come from.
