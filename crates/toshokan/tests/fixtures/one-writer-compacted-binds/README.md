# one-writer-compacted-binds

As `one-writer-compacted`, by a build whose snapshot lists in `binds` the folded `bind` entries whose pins it holds.

Written after toshokan 0.1.0 with `cargo test -p toshokan --test format -- --ignored --exact generate::fixtures`, from `tests/format/generate.rs`: seed 1, a `TestClock` from 1000 ms, `PrefixIdentity` over 64 KiB and the layout `.t`.
