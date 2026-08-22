//! What the session driver does that no operation names.
//!
//! The exchanges themselves live in `tests/scripts` and are replayed by `tests/replay`,
//! which drives each script through the intent its header declares. What is left here is
//! the handful of behaviours an intent cannot express: a notification arriving in place
//! of a reply, the drain that has to be capped, a stale session cleared and the open
//! retried. Each reads its script from the same committed tree, so nothing in this file
//! is a pasted exchange either.
//!
//! No hardware, no platform dependency: runs anywhere the crate compiles.

#![cfg(feature = "replay")]

#[path = "support/scripts.rs"]
mod scripts;

use nord_usb::transport::ReplayTransport;
use nord_usb::wire::ObjectClass;
use nord_usb::Session;

fn replaying(name: &str) -> ReplayTransport {
    ReplayTransport::new(scripts::fixture(name).steps())
}

/// A close that the device refuses must surface as the `Err` it is — and must still
/// release the UI session.
///
/// `commit()` consumes the session, so a failure inside it drops the session mid-close;
/// if `closed` were only marked on success, the debug `Drop` assertion would panic over
/// the very error the caller was owed — in exactly the always-close error paths the CLI
/// is built around. And the `HELLO` is the half that wedges the instrument, so the
/// refused `SESSION_CLOSE` must not skip the `GOODBYE`: the script ends with that
/// exchange, so `is_exhausted` is the assertion that it was sent.
#[test]
fn a_failed_commit_reports_rather_than_panicking() {
    let mut t = replaying("session/refused_close.script");
    let err = pollster::block_on(async {
        let s = Session::open(&mut t, ObjectClass::Program).await.unwrap();
        s.commit().await.expect_err("the device refused the close")
    });
    assert!(
        matches!(err, nord_usb::Error::DeviceStatus(5)),
        "wrong error: {err}"
    );
    assert!(
        t.is_exhausted(),
        "the refused close did not send GOODBYE, leaving the device half-open"
    );
}

/// A queued notification must be drained — the real reply read out from behind it —
/// and surfaced, not mistaken for the reply. Mistaking it failed the open *and*
/// wedged the instrument on hardware: the mismatch bailed without the drain, and the
/// session never recovered.
#[test]
fn an_unsolicited_changed_notification_is_drained_not_mistaken_for_the_reply() {
    let mut t = replaying("session/changed_notification.script");
    pollster::block_on(async {
        let s = Session::open(&mut t, ObjectClass::Program).await.unwrap();
        assert!(
            s.instrument_changed(),
            "the drained notification must be surfaced, not silently skipped"
        );
        s.commit().await.unwrap();
    });
    assert!(t.is_exhausted(), "did not consume the whole exchange");
}

/// The drain is capped: a device streaming notifications must not pin the host in the
/// read loop forever. Past [`nord_usb::session::DRAIN_CAP`] the notification is
/// reported as the unexpected response it is — and that bail still says GOODBYE.
#[test]
fn a_notification_flood_bails_rather_than_looping() {
    let mut t = replaying("session/notification_flood.script");
    let err = pollster::block_on(async {
        match Session::open(&mut t, ObjectClass::Program).await {
            Ok(s) => {
                s.abort();
                panic!("a flood of notifications was reported as a successful open");
            }
            Err(e) => e,
        }
    });
    assert!(
        matches!(err, nord_usb::Error::UnexpectedResponse { got: 0x2c, .. }),
        "wrong error: {err}"
    );
    assert!(
        t.is_exhausted(),
        "the flood bail did not send GOODBYE, leaving the device half-open"
    );
}

/// A session the device still thinks is open must be cleared and the open retried.
///
/// This is the wedge that looked like a hardware fault: every operation is wrapped in a
/// session, so when the device refuses to open one with `0x12`, nothing built on
/// [`Session`] can reach it — including the single frame that fixes it. The recovery is a
/// bare `SESSION_CLOSE`, and the script asserts it is sent *between* the refused open and
/// a successful retry.
#[test]
fn a_stale_session_is_cleared_and_the_open_retried() {
    let mut t = replaying("session/stale_session_retried.script");
    pollster::block_on(async {
        let s = Session::open(&mut t, ObjectClass::Program)
            .await
            .expect("a stale session should have been cleared and the open retried");
        s.commit().await.expect("close");
    });
    assert!(
        t.is_exhausted(),
        "the recovery did not send a bare SESSION_CLOSE before retrying the open"
    );
}
