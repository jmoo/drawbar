# Format fixtures

Libraries written by toshokan, kept byte for byte. Once a build that wrote them is
released, folders hold these bytes, and every later build must read them the same
way. `tests/format` holds that contract.

Each state is a directory:

- `folder/` and `local/` are the library folder and the install's local root.
- `empty-dirs.json` lists the directories of each that hold nothing, which git
  cannot keep.
- `view.oracle.json` is what a reader shows once it has scanned every file. It is
  written by hand from what the generator did.
- `README.md` says what the state holds and how it was written.

The `format` suite opens each state with its local root and as a fresh install,
compares what it shows with the oracle fact by fact, and checks that this build
encodes every line, snapshot, pending record, `head.json` and `identities.json`
back to the same bytes.

`reverse/` holds what a newer build might write: files laid over a state, written
by hand. Each case's test states what this build does with them.

## Never regenerate

A state is never written again. A change to what toshokan writes adds a new state
beside the old ones, and the old ones must still read as their oracles say, or
the change needs a migration.

To add a state, add it to `tests/format/generate.rs` and run:

```sh
cargo test -p toshokan --test format -- --ignored --exact generate::fixtures
```

The generator writes only the states and reverse cases that are not here yet.
Then write the state's `view.oracle.json` by hand from what the generator did, add
the state to the `format` suite, and commit both.
