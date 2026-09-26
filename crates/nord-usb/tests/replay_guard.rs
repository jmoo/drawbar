//! A test target that compiles under every feature set, the counterpart of
//! `nord-format/tests/corpus_guard.rs`.
//!
//! ⚠️ The corpus replays and the enumeration walks only build with the `corpus` feature,
//! which implies `replay`. Without it, `cargo test -p nord-usb` compiles them out and
//! passes without replaying any recorded exchange.

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
