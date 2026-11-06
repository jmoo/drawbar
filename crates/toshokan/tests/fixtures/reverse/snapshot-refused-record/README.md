# snapshot-refused-record

A second snapshot of the writer, `snapshot-newer.json`, whose first record holds a member this build does not know.

Laid over the state `one-writer-compacted`. Its bytes are written by hand in `tests/format/generate.rs`, each line sealed by toshokan 0.1.0 with `cargo test -p toshokan --test format -- --ignored --exact generate::fixtures`.
