# Synthetic fixtures

Specimen files written by this crate's own writers — no instrument bytes, no
vendor material — so the sweep in `tests/corpus` has a tree to read in any
checkout, with no corpus. They follow the corpus's conventions: a file joins by
being readable, and `<file>.oracle.json` beside one says what was set.

- `cbin/<tag>.g<N>.cbin` — every registered CBIN tag in both header
  generations, zero body: the container is the specimen.
- `ne5/` — Electro 5 entities built through the public constructors and
  setters: defaults, one mutated program per panel, a settings edit, a song,
  each edit pinned by its sidecar.
- `nsmpproj/` — Sample Editor projects from `nsmpproj::Project::new`: one
  zone, three, and three with the middle zone retuned and its range moved,
  each pinned by its sidecar.

The files are golden: `tests/fixtures.rs` regenerates the same bytes and
compares, so a writer change shows up as a byte diff in git rather than moving
silently with the code. After an intentional writer change:

```sh
UPDATE_FIXTURES=1 cargo test -p nord-format --test fixtures
```

and read the diff.
