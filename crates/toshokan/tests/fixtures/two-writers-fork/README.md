# two-writers-fork

Writers `a` and `b` naming each other's writes. `a`'s history forks three ways after a copy of its local root: two branches compacted into tips, one left as a line. `b` takes from the line's run and dismisses one tip's run, compacts, then takes from the other tip's run, dismisses the line's run and writes once more, its segment left open. The local root is `b`'s.

Written by toshokan 0.1.0 with `cargo test -p toshokan --test format -- --ignored --exact generate::fixtures`, from `tests/format/generate.rs`: seeds 11 to 18, a `TestClock` from 1000 ms, `PrefixIdentity` over 64 KiB and the layout `.t`.
