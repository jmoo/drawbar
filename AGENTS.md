# AGENTS.md

Rust tools for reading, editing, and moving programs on Clavia Nord
instruments: reverse-engineered file formats (`nord-format`), the USB vendor
protocol (`nord-usb`), a CLI (`nord-cli`), the bitfield derive backing them
(`nord-bits-derive`), and a WebUSB demo (`nord-web-demo`).

> **This is a public, open-source repo.** Keep personal information, secrets,
> and Claude's persistent memory out of it. Private notes, RFCs, and the
> specimen corpus live elsewhere; if you wouldn't post it publicly, it doesn't
> go in this repo.

Not affiliated with, authorized, or endorsed by Clavia DMI AB — keep the
trademark disclaimers in the crate READMEs and docs intact.

## Reverse engineering rules

All reverse engineering is **blackbox**, for interoperability:

- **No decompilation or disassembly** of Clavia's software or firmware. Formats
  and protocol come from specimen files, captured USB traffic, and hardware
  observation on instruments the author owns.
- **No license violations.** Do not copy code, tables, or data from projects
  without a permissive license. Documentation under a permissive license may be
  used with credit (e.g. Chris55's `nord-documentation`, BSD-3). A source with
  no clear license may be *read* but not reproduced — values only documented
  there stay raw integers rather than named enums.
- **Public or leaked documentation may be consulted.** Consulting a document is
  fine; copying its text or embedding its data verbatim is a licensing
  question, per the rule above.

## Copyright material

- **No Clavia proprietary material in this repo** — no factory presets, piano
  or sample libraries, firmware images, or files shipped with Clavia software.
  Specimen files checked in here must be self-generated (parameter sweeps
  written by our own tools or the instrument's panel). The specimen corpus,
  which does contain proprietary sample data, lives in the **private**
  `jmoo/nord-corpus` repo and is only reachable through the `corpus` cargo
  feature + `NORD_CORPUS_DIR`.
- **No code from non-permissively-licensed projects.** BSD/MIT/Apache with
  attribution is fine; GPL/unlicensed is not.

## Commands

Run cargo from `crates/` — `.cargo/config.toml` is discovered by walking up
from the working directory, and wasm builds need its
`--cfg=web_sys_unstable_apis` flag. 

DO NOT ASSUME ANY TOOL IS INSTALLED. 

This is a nix native project. Cargo needs to be executed from the nix devshell.
If you reach for any other tools (python, jq, etc.) invoke them with nix shell if
they are not installed globally.

- Test: `cargo test --workspace` (from `crates/`). CI runs
  `cargo test -p nord-bits-derive -p nord-format --features nord-format/bundle`.
- Corpus tests: `--features corpus` with `NORD_CORPUS_DIR` pointing at a
  `nord-corpus/ne5` checkout; without the private corpus they don't compile in,
  and the default suite must keep passing anywhere.
- Nix: `nix build .#<crate>` builds and tests one crate;
  `nix flake check` adds the cross-compile targets and the corpus check.
- Format: `nix fmt` formats the whole tree; `nix flake check` fails on anything
  it would have changed.

## Releasing

`nord-bits-derive` and `nord-format` are published to crates.io. Pushing a
`<crate>-v<version>` tag triggers `.github/workflows/release.yml` (crates.io
Trusted Publishing — no token secrets). Bump the crate's own `version` first;
the other crates share the workspace version and are unpublished.

## Code style

- **Format before committing**: `nix fmt`. treefmt drives rustfmt for Rust,
  nixfmt (2-space) for `.nix` and taplo for `.toml`; the versions come from the
  flake, so a locally installed formatter is neither needed nor authoritative.
- Nix, style-wise: alphabetize attribute-set keys; single child → dotted path
  (`a.b.c = v;`), 2+ children → nested braces. Decide per attrset literal.
- **The round-trip invariant is load-bearing**: decoded values are views over a
  verbatim body, bits no field claims survive untouched, and
  `to_bytes(from_stream(x)) == x` bit-for-bit. Don't trade it away for a nicer
  API.

## Comments

Applies to Rust, shell and nix alike.

**A comment states the invariant or the surprise. Git states the history.**
Most bad comments are the second one standing in the first one's place.
Comments should be rare, useful, and concise

**Earns a comment**

- **Non-obvious encodings** — `// perc speed stores 2/1/3 for soft/fast/both`.
- **Hazards** — a correct-looking call that is wrong in some state. Lead with `⚠️`.
- **Invariants** a reader would otherwise break, especially ones the type
  system can't hold.

**Delete on sight**

- Any sentence whose subject is a past version of the code — "earlier", "used
  to", "this previously". `git blame` holds it, and a reader who trusts it as
  current is misled.
- Design rationale: why this shape was chosen over another. State what is true;
  the comparison is internal and the reader can't act on it.
- Restating the next line.
- Narrating the work: "now handle the edge case", "fixed the bug where…".

**Mark provenance.** In reverse-engineered code a guess presented as a fact
costs hours downstream. Three states, three phrases:

```rust
// Confirmed on hardware.
// Inferred from specimens; not confirmed on hardware.
// Unexplained: real programs hold this, and the panel cannot produce it.
```

Never launder an inference into a statement by dropping the qualifier. Keep
counts and dates out — the corpus grows and the comment silently goes wrong.
Arithmetic that *shows* an encoding is not a measurement and is welcome:
`// 43/127*10 = 3.39, and the panel reads 3.4`.

**Don't reference private notes.** RFCs and format notes live in a private
vault; a pointer to them is dead to every reader of this repo. If a reader
needs the fact, put the fact in the code. Public references — an upstream issue
or discussion — are fine.

**`///` vs `//`**

- `///` is the **contract** — what a caller may rely on, written for someone
  who will never read the body. If it explains *how*, it probably wants to be `//`.
- `//` is the **surprise inside the body** — written for someone already
  reading the code who would otherwise stop and ask why.

**Density is a smell, not a target.** Roughly, a file past ~20% comment lines
is usually carrying exposition. Format and protocol files legitimately run
higher — the bytes genuinely need explaining. Application code does not.
Investigate, don't reflexively cut: deleting a hazard to hit a number is worse
than the exposition was.

## Working in this repo

- **Commit in small, logical commits, and only when asked.** Branch off
  `master` first if needed.
- Refactors must be behavior-preserving — the default test suite and (where
  available) the corpus round-trip are the proof.
