# Contributing

**What is this?**

Rust tools for interacting with Clavia Nord instruments: 
 - reverse-engineered file formats (`nord-format`)
 - the USB vendor protocol (`nord-usb`)
 - a CLI (`nord-cli`)
 - the bitfield derive backing them (`nord-bits-derive`)
 - and an egui app for the desktop and the browser (`drawbar`)

> **This is a public, open-source repo.** Keep personal information, secrets,
> and agent persistent memory out of it. Private notes, RFCs, and the
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
  without a permissive license.
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
  feature + `NORD_CORPUS_ROOT`. You can test against your own corpus using
  that envvar if you do not have access to jmoo/nord-corpus. See
  `crates/nord-format/tests/fixtures` for an example corpus layout.
- **No code from non-permissively-licensed projects.** BSD/MIT/Apache with
  attribution is fine; GPL/unlicensed is not.

## Commands

Run cargo from `crates/` — `.cargo/config.toml` is discovered by walking up
from the working directory, and wasm builds need its
`--cfg=web_sys_unstable_apis` flag.

DO NOT ASSUME ANY TOOL IS INSTALLED GLOBALLY.

This is a nix native project. Cargo needs to be executed from the nix devshell.
If you reach for any other tools (python, jq, etc.) invoke them with nix shell
if they are not installed globally.

- Test: `cargo test --workspace` (from `crates/`). CI tests `nord-bits-derive`,
  `nord-format`, `nord-usb` and `nord-cli` with each crate's `testFeatures`, and
  fails on anything `nix fmt` would change or clippy would flag.
- Corpus tests: `--features corpus` with `NORD_CORPUS_ROOT` pointing at a
  `nord-corpus` checkout; without the private corpus they don't compile in, and
  the default suite must keep passing anywhere. `tests/corpus` generates one test
  per readable file — the committed `tests/fixtures/` always, the corpus with the
  feature — and asserts the `<file>.oracle.json` sidecar where one exists.
- Nix: `nix build .#<crate>` builds and tests one crate, `.#nord.all` every
  crate and cross target. Everything lives flat under the `nord` attribute;
  `packages` exposes the crates and cross builds. `nix flake check` verifies
  formatting and that every output evaluates.
- Corpus in Nix: `nix build .#nord.all-corpus` runs every crate's suite against
  the pinned corpus (`.#nord.<crate>-corpus` for one of them).
  `.#nord.all-corpus-full` swaps in the R2 tier, which needs a store seeded by
  `corpus nix-add` or R2 credentials in the builder; `.#nord.corpus` and
  `.#nord.corpus-full` are the assemblies themselves. The corpus repo is
  private, so evaluating any corpus attr needs read access to it.
- Format: `nix fmt` formats the whole tree; `nix flake check` fails on anything
  it would have changed.

## Commits & merges

- **Linear history: no merge commits.** Integrate work into your branch with
  rebase; integrate branches into `master` with squash or fast-forward merges.
  CI fails any PR whose commits contain a merge commit.
- **Every commit title is a Conventional Commit**:

  ```
  <type>(<optional scope>): description
  ```

  Types: `feat`, `fix`, `docs`, `style`, `refactor`, `perf`, `test`, `build`,
  `ci`, `chore`, `revert`. The scope is usually the crate name, e.g.
  `fix(nord-format): …`. `!` before the colon — or a `BREAKING CHANGE:` footer
  in the body — marks a breaking change.
- **PR titles follow the same rule.** With squash merges the PR title becomes
  the commit title on `master`, so it is the one that counts.
- **Versions bump from commits.** `scripts/bump.bash` reads each crate's
  Conventional Commits since its last release tag: a breaking change (`!` or a
  `BREAKING CHANGE:` footer) bumps the major (the minor while the crate is
  0.x), `feat` the minor, `fix`/`perf`/`revert` the patch; anything else
  releases nothing. Only commits touching the crate's directory count, and a
  crate whose dependency was bumped gets at least a patch bump.

## Releasing

Every crate is published to crates.io, each with its own version in its
`Cargo.toml` — the only place versions live. Both scripts take `--dry-run`
to show what they would do; run them from the dev shell, which has their
tools.

1. **Bump**: put the `bump` label on the PR. `bump.yml` runs
   `scripts/bump.bash --title` with the PR title — the squashed commit that
   will land — applies its level to the crates the diff touches (plus at
   least a patch for their dependents), refreshes `Cargo.lock` and pushes
   the `chore(release): …` commit. It re-runs on every push and retitle
   while the label is on, converging from the merge-base versions, so a
   stale bump corrects itself. Fork PRs can't be pushed to: run
   `scripts/bump.bash --title '<pr title>'` locally instead. Plain
   `scripts/bump.bash` (no `--title`) is the catch-up mode: run on master,
   it bumps each crate from its commits since its last release tag.
2. **Release**: on every push to `master`, `release.yml` runs
   `scripts/release.bash`, which publishes each crate whose version has no
   `<crate>-v<version>` tag yet (Trusted Publishing via OIDC — no token
   secrets) and creates that tag as a GitHub release. The notes are the
   crate's commits since its previous tag, grouped into breaking changes,
   features, bug fixes, performance and other. Dependencies go before
   dependents. Idempotent: a failed run is fixed by re-running it from the
   Actions tab.

First-ever publish of a new crate can't use Trusted Publishing (it is
configured on an already-existing crate) — `cargo publish` it locally once;
the scripts pick it up from `cargo metadata` with no further wiring.

## Code style

- **Format before committing**: `nix fmt`. treefmt drives rustfmt for Rust,
  nixfmt (2-space) for `.nix`, taplo for `.toml` and shfmt for shell; the
  versions come from the flake, so a locally installed formatter is neither
  needed nor authoritative.
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

**Prefer 1-2 line comments most of the time (or no comment at all)** Exceptions 
include top of file documentation intended for docgen viewing.

## Working in this repo

- **Commit in small, logical commits, and only when asked.** Branch off
  `master` first if needed.
- Refactors must be behavior-preserving — the default test suite and (where
  available) the corpus round-trip are the proof.
