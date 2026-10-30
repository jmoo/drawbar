# snapshot-unknown-entries

The writer's snapshot holding an op this build does not know, as a newer build compacts one.

Laid over the state `one-writer-compacted`. Its bytes are written by hand in `tests/format/generate.rs`, each line sealed by toshokan 0.1.0 with `cargo test -p toshokan --test format -- --ignored --exact generate::fixtures`.
