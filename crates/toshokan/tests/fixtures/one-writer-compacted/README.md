# one-writer-compacted

The history of `one-writer-open`, compacted into a snapshot, then one more intent in a segment sealed at close, whose compaction failed to stage its snapshot.

Written by toshokan 0.1.0 with `cargo test -p toshokan --test format -- --ignored --exact generate::fixtures`, from `tests/format/generate.rs`: seed 1, a `TestClock` from 1000 ms, `PrefixIdentity` over 64 KiB and the layout `.t`.
