//! The one test target that compiles under every feature set — the twin of
//! `nord-format/tests/corpus_guard.rs`.
//!
//! ⚠️ The golden replays are the only check on the encoder, and they are
//! `#![cfg(feature = "replay")]`. A bare `cargo test -p nord-usb` compiles them
//! out and passes having verified none of the wire encoding.

/// A corpus variable set with the gate off means no wire test ran. The retired
/// `NORD_CORPUS_DIR` name counts too: it is stale documentation, still intent.
#[test]
fn replay_gate_off_means_the_wire_encoding_is_unverified() {
    #[cfg(not(feature = "replay"))]
    assert!(
        std::env::var_os("NORD_CORPUS_ROOT").is_none()
            && std::env::var_os("NORD_CORPUS_DIR").is_none(),
        "a corpus variable is set but --features replay is off: no wire test ran. The \
         full command is\n    \
         cargo test --workspace --features nord-usb/replay,nord-format/corpus"
    );
}
