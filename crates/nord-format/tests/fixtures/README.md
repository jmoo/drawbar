# Synthetic fixtures

Specimen files written by this crate's own writers — no instrument bytes, no
vendor material — so the read → write round trip runs against real files on
disk in any checkout, with no corpus.

- `cbin/<tag>.g<N>.cbin` — every registered CBIN tag in both header
  generations, zero body: the container is the specimen.
- `ne5/` — Electro 5 entities built through the public constructors and
  setters: defaults, one mutated program per panel, a settings edit, a song.

The files are golden: `tests/fixtures.rs` regenerates the same bytes and
compares, so a writer change shows up as a byte diff in git rather than moving
silently with the code. After an intentional writer change:

```sh
UPDATE_FIXTURES=1 cargo test -p nord-format --test fixtures
```

and read the diff.
