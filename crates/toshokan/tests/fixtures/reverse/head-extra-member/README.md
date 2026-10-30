# head-extra-member

The live writer's `head.json` with a member this build does not know.

Laid over the state `local`. Its bytes are written by hand in `tests/format/generate.rs`, each line sealed by toshokan 0.1.0 with `cargo test -p toshokan --test format -- --ignored --exact generate::fixtures`.
