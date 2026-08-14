//! The one test target that compiles under every feature set.
//!
//! ⚠️ The corpus suites (`tests/corpus`, `decode_sanity.rs`, `decode_snapshot.rs`)
//! need `--features corpus`; without it they compile out (or are skipped by
//! `required-features`) and `cargo test` reports a pass having verified none of
//! the decode. A set `NORD_CORPUS_ROOT`/`NORD_CORPUS_DIR` is someone saying
//! they meant to run them.

/// A corpus variable set with the feature off means the corpus suites did not run.
#[test]
fn corpus_env_without_the_corpus_feature_is_a_mistake() {
    #[cfg(not(feature = "corpus"))]
    assert!(
        std::env::var_os("NORD_CORPUS_ROOT").is_none()
            && std::env::var_os("NORD_CORPUS_DIR").is_none(),
        "a corpus variable is set but --features corpus is off: the corpus suites did \
         not run and this run verified none of them. The full command is\n    \
         cargo test --workspace --features nord-usb/replay,nord-format/corpus"
    );
}
