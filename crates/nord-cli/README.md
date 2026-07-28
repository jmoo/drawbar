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
| `program` | Work with programs on an attached instrument |
| `device` | Lower-level device access, any object class |

Slots are written **`BANK:SLOT`**, the way the instrument and Nord Sound Manager
show them — `7:4` is bank 7, slot 4, both counted from 1. (`7-4` also parses,
which is what the older `device` help documented.)

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
values the instrument reports for that program over USB, so `nord device deps` on
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

Songs list their four program slots; settings print the raw body (field decode is
still pending); bundles need the `bundle` feature (enabled here) to open.

### `verify`

Parses each file, writes it back, and checks the bytes are identical — reporting
the offset of the first difference if not. It is the only thing that exercises
the write path end to end:

```
$ nord verify song.ne5t settings.ne5s
ok     song.ne5t (62 bytes)
error  settings.ne5s (ne5s: re-encoded to 24 bytes but the format is 78; refusing to emit a truncated file)
error: 1 of 2 file(s) did not round-trip
```

## Working with an instrument

Close Nord Sound Manager first — it claims the vendor interface exclusively, and
`nord` cannot attach alongside it.

```sh
nord program get 7:4                    # summary to stdout
nord program get 7:4 -o patch.ne5p      # write the .ne5p instead
nord program put patch.ne5p 7:4 --yes
nord program move 8:13 7:16 --yes
nord program delete 7:50 7:49 --yes
```

`nord device` reaches the other object classes through `--class` (4 programs, the
default; 5 set lists; 1 pianos; 3 samples) and adds a few operations:

```sh
nord device status                      # inventory per class; --json for machines
nord device info 1:1 --class 1          # size, format, version, name, crc32
nord device deps 7:4                    # piano/sample dependencies, with names
nord device select 2:12                 # load live on the instrument
nord device read 1:1 --class 5 --raw    # wire body verbatim, no CBIN header
nord device rename 6:13 "foo" --yes
nord device duplicate 7:2 7:3 --yes
```

`--raw` matters for formats whose header layout is not known: wrapping the body
in a fabricated header would produce a plausible-looking file that is wrong.

### Safety

Every mutating command **reads the slot first, says what it will touch, then
refuses without `--yes`**. Running it without the flag is a real dry run:

```
$ nord program move 7:2 7:3
moving "Africa Split" from bank 7 slot 2 to bank 7 slot 3 — OVERWRITING "Squabble B"
error: refusing to modify the device without --yes (back up first: `nord device read`)
```

`move` and `duplicate` name the *destination's* current occupant, not just the
source — that is the thing about to be lost.

Two other guards worth knowing about:

- An empty slot reports `bank 5 slot 42 is empty` rather than a raw status code.
- Every command closes its transaction **even when it fails**. An abandoned
  session leaves the instrument stuck on a progress screen with no way out but a
  power cycle.

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
