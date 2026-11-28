# one-writer-joined

One writer whose entity `built` names three others in `sources`. It joins `second` into `low` and deletes `gone`, compacts into a snapshot that holds the join, then joins `third` into `low` in a segment sealed at close, whose compaction failed to stage its snapshot.

Written by toshokan 0.1.1 with `cargo test -p toshokan --test format -- --ignored --exact generate::fixtures`, from `tests/format/generate.rs`: seed 51, a `TestClock` from 1000 ms, `PrefixIdentity` over 64 KiB and the layout `.t`.
