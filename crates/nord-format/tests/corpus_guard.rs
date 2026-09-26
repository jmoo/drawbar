//! ⚠️ The corpus suites (`corpus_behaviors.rs`, `codec_behaviors.rs`,
//! `coverage.rs`, and the corpus half of `tests/corpus`) need `--features
//! corpus`. Without it they compile out, and `cargo test` passes without having
//! checked them. Setting `NORD_CORPUS_ROOT` says the caller meant to run them.

#[test]
fn corpus_env_without_the_corpus_feature_is_a_mistake() {
    #[cfg(not(feature = "corpus"))]
    assert!(
        std::env::var_os("NORD_CORPUS_ROOT").is_none(),
        "NORD_CORPUS_ROOT is set but --features corpus is off, so the corpus suites \
         did not run. To run them:\n    \
         cargo test --workspace --features nord-usb/replay,nord-format/corpus"
    );
}
