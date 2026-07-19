# nord-cli

A thin command-line tool (`nord`) over the [`nord-format`](../nord-format)
library. Its job is to **dogfood the library** and let you eyeball what a Nord
file decodes to — not to be a product.

## Usage

```sh
# Summary of a program, song, settings, piano, sample, or backup bundle:
nord inspect path/to/patch.ne5p

# Several at once:
nord inspect *.ne5p

# Full decoded structure (Debug dump):
nord inspect --raw path/to/song.ne5t
```

`inspect` prints a readable summary per file (and exits non-zero if any file
fails to parse). For an Electro 5 program it shows the parts, split, transpose,
part mix, gain, and — when the organ is in use — the per-model organ state:

```
program.ne5p
  type:      Electro 5 program (ne5p)
  location:  bank 8 slot 23
  lower:     Organ  octave +0  sustain yes  control no
  upper:     Organ  octave +0  sustain yes  control no
  split:     no
  transpose: +1  (no)
  part mix:  49.6/50.0 (lower/upper %)
  gain:      127
  organ:     drawbars / vibrato / percussion, selected preset per model
    b3   p1  000000000  vib off  perc Both
    vox  p1  888800000  vib V3
    farf p1  800000000  vib off
    pipe p1  800000000
```

Songs list their four program slots; settings print the raw body (field decode
is still pending); bundles need the `bundle` feature (enabled here) to open.

## Build & run

From the lab workspace:

```sh
nix develop
cd crates
cargo run -p nord-cli -- inspect path/to/patch.ne5p
```

Or as a Nix package — `nix build .#nord-cli` installs the `nord` binary.
