//! The one test target that compiles under every feature set — the twin of
//! `nord-format/tests/corpus_guard.rs`.
//!
//! ⚠️ The corpus replays and the enumeration walks are `#![cfg(feature = "corpus")]`,
//! and `corpus` implies `replay`. A `cargo test -p nord-usb` without it compiles them
//! out and passes having replayed none of the recorded exchanges.

/// The corpus variable set with the gate off means no recorded exchange ran.
#[test]
fn the_corpus_gate_off_means_the_recorded_exchanges_are_unverified() {
    #[cfg(not(feature = "corpus"))]
    assert!(
        std::env::var_os("NORD_CORPUS_ROOT").is_none(),
        "NORD_CORPUS_ROOT is set but --features corpus is off: no recorded exchange ran. \
         The full command is\n    \
         cargo test --workspace --features nord-usb/corpus,nord-format/corpus"
    );
}
