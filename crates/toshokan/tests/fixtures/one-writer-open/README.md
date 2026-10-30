# one-writer-open

One writer's log, its last segment left open by an instance that stopped without closing, and its local root.

Written by toshokan 0.1.0 with `cargo test -p toshokan --test format -- --ignored --exact generate::fixtures`, from `tests/format/generate.rs`: seed 1, a `TestClock` from 1000 ms, `PrefixIdentity` over 64 KiB and the layout `.t`.
