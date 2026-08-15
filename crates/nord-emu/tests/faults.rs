//! The failure states, which are the reason this crate exists.
//!
//! Every one of these cost a power cycle or an afternoon to find on hardware, and none
//! of them can be scripted: they are what the device *does next*, given what it was
//! asked before.

mod common;

use std::time::Duration;

use common::*;
use nord_emu::{EmuDevice, Object};
use nord_usb::wire::{cmd, Location, ObjectClass, Service};
use nord_usb::{block_on, session};
use nord_usb::{op, Error, Session};

fn program(name: &str) -> Object {
    Object::new(name, b"ne5p", 4, vec![0; 121])
}

/// A session an earlier run abandoned refuses the next one with `0x12`, and the cure is
/// a frame no session can carry — which is why [`Session::open`] sends it bare.
#[test]
fn a_stale_class_session_is_cleared_and_the_open_retried() {
    let mut device = EmuDevice::new();
    device.hold_session(ObjectClass::Program);
    let mut t = transport(device);

    block_on(async {
        let s = Session::open(&mut t, ObjectClass::Program)
            .await
            .expect("a stale session should have been cleared and the open retried");
        s.commit().await.unwrap();
    });

    // Refused open, bare SESSION_CLOSE, accepted open — in that order.
    let opens = t
        .sent()
        .iter()
        .filter(|f| f.get(12..16) == Some(&[0, 0, 0, cmd::SESSION_OPEN as u8]))
        .count();
    assert_eq!(opens, 2, "the open was not retried exactly once");
}

/// The dangerous wedge: an abandoned UI session makes every slot in every class read
/// empty, and nothing fails.
///
/// ⚠️ It answers *successfully*, so nothing at the protocol level distinguishes the lie
/// from a genuinely vacant slot — and it survives reopening the session. A bare
/// `GOODBYE` is what cures it.
#[test]
fn an_abandoned_ui_session_makes_every_slot_read_empty_until_recovery() {
    let at = Location::from_user(7, 4);
    let mut device = EmuDevice::new();
    device.insert(ObjectClass::Program, at, program("still here"));
    device.abandon_ui_session();
    let mut t = transport(device);

    block_on(async {
        let mut s = Session::open(&mut t, ObjectClass::Program).await.unwrap();
        let lie = op::info(&mut s, at).await;
        assert!(
            matches!(lie, Err(Error::DeviceStatus(1))),
            "the wedge should report the occupied slot as empty"
        );
        s.commit().await.unwrap();

        // A whole clean session does not clear it — its own GOODBYE only balances its
        // own HELLO.
        let mut s = Session::open(&mut t, ObjectClass::Program).await.unwrap();
        assert!(
            op::info(&mut s, at).await.is_err(),
            "the wedge cleared early"
        );
        s.commit().await.unwrap();

        // The cure, sent bare: no session wrapped around it.
        op::recover(&mut t).await.unwrap();

        let mut s = Session::open(&mut t, ObjectClass::Program).await.unwrap();
        assert_eq!(op::info(&mut s, at).await.unwrap().name, "still here");
        s.commit().await.unwrap();
    });
}

/// A front-panel STORE queues an unsolicited notification, which arrives in place of
/// whatever reply the host reads for next. It must be drained and surfaced, not mistaken
/// for the reply.
#[test]
fn an_unsolicited_change_notification_is_drained_and_surfaced() {
    let mut device = EmuDevice::new();
    device.queue_changed();
    let mut t = transport(device);

    block_on(async {
        let mut s = Session::open(&mut t, ObjectClass::Program).await.unwrap();
        assert!(
            s.instrument_changed(),
            "the notification must be surfaced, not silently skipped"
        );
        // And the session survives it, rather than desyncing.
        op::status(&mut s).await.unwrap();
        s.commit().await.unwrap();
    });
}

/// Not every command replies, and a host that assumes one hangs.
///
/// `probe` bounds the read for exactly this: an unrecognised code the device ignores
/// comes back as "it said nothing", and the transaction still closes.
#[test]
fn a_command_the_device_ignores_answers_nothing_and_still_closes() {
    let mut t = transport(EmuDevice::new());
    block_on(async {
        let mut s = Session::open(&mut t, ObjectClass::Program).await.unwrap();
        let answer = s
            .probe(Service::Program, 10, 0x3b, &[], Duration::from_millis(1))
            .await
            .unwrap();
        assert!(answer.is_none(), "the device answered a code it ignores");
        s.commit().await.unwrap();
    });
}

/// ⚠️ `0x7e` paints "Deleting..." on the display, answers nothing at all, and leaves the
/// session impossible to close.
///
/// Modelled so a host can be tested against a device that has stopped talking mid
/// transaction — the close has to report rather than hang or panic.
#[test]
fn the_command_that_never_answers_leaves_the_session_unclosable() {
    let mut t = transport(EmuDevice::new());
    let err = block_on(async {
        let mut s = Session::open(&mut t, ObjectClass::Program).await.unwrap();
        s.set_read_limit(Duration::from_millis(1));
        let answer = s
            .probe(
                Service::Program,
                10,
                cmd::DO_NOT_SEND_DELETING,
                &[],
                Duration::from_millis(1),
            )
            .await
            .unwrap();
        assert!(answer.is_none());
        s.commit().await.expect_err("the close cannot be answered")
    });
    assert!(matches!(err, Error::Transport(_)), "wrong error: {err}");
    assert!(t.device().stopped());
}

/// ⚠️ A frame the device cannot handle stalls the bulk endpoints, and the same stall
/// comes from two unrelated codes in two services — so it is a general response rather
/// than a property of either, and any unprobed code can do it.
///
/// Nothing stored is harmed; the instrument keeps playing and only a power cycle clears
/// it. What a host can do about it is notice, which is what this test is about.
#[test]
fn a_frame_the_device_cannot_handle_stalls_the_endpoints() {
    let mut t = transport(EmuDevice::new());
    let err = block_on(async {
        let mut s = Session::open(&mut t, ObjectClass::Program).await.unwrap();
        s.set_read_limit(Duration::from_millis(1));
        // The subsystem field is a protocol version, and probing it downward is what
        // stalled a real instrument.
        let err = s
            .probe(
                Service::Program,
                1,
                cmd::STATUS,
                &[],
                Duration::from_millis(1),
            )
            .await;
        assert!(
            matches!(err, Ok(None)),
            "a stalling frame draws no reply, not an error"
        );
        s.commit()
            .await
            .expect_err("nothing can be written to a stalled endpoint")
    });
    assert!(
        err.to_string().contains("power cycle"),
        "the error should say what the operator has to do: {err}"
    );
    assert!(t.device().endpoints_stalled());
}

/// A stalled endpoint stops accepting **writes**, which no read timeout can catch: the
/// host blocks before any read is reached.
#[test]
fn stalled_endpoints_are_reported_rather_than_blocking_forever() {
    let mut device = EmuDevice::new();
    device.stall_endpoints();
    let mut t = transport(device);

    let err = block_on(async {
        match Session::open(&mut t, ObjectClass::Program).await {
            Ok(s) => {
                s.abort();
                panic!("a stalled device reported a successful open");
            }
            Err(e) => e,
        }
    });
    assert!(matches!(err, Error::Transport(_)), "wrong error: {err}");
    assert!(
        err.to_string().contains("power cycle"),
        "the error should say what the operator has to do: {err}"
    );
}

/// Any mutation since power-up progressively disables the enumeration cursor, and only a
/// power cycle brings it back. Every other operation stays healthy.
///
/// The dangerous failure would be a walk that returns what it had and looks complete;
/// the refusal has to propagate.
#[test]
fn a_mutation_poisons_the_cursor_and_the_walk_reports_it() {
    let at = Location::from_user(1, 1);
    let mut device = EmuDevice::new();
    device.insert(ObjectClass::Program, at, program("first"));
    device.poison_cursor_on_mutation(true);
    let mut t = transport(device);

    block_on(async {
        let mut s = Session::open(&mut t, ObjectClass::Program)
            .await
            .unwrap()
            .allow_destructive_writes();
        // A same-name rename is enough on hardware: no occupancy change, no visible
        // change at all.
        op::rename(&mut s, at, "first").await.unwrap();

        let walk = op::occupied_slots(&mut s, 500).await;
        assert!(
            matches!(walk, Err(Error::DeviceStatus(op::ENUMERATION_DISABLED))),
            "a refused walk must not come back as a partial list"
        );

        // Point commands are unaffected — that is what makes it a quirk of the
        // enumeration subsystem rather than a wedge.
        assert_eq!(op::info(&mut s, at).await.unwrap().name, "first");
        op::status(&mut s).await.unwrap();
        s.commit().await.unwrap();
    });
    assert!(t.device().mutated());
}

/// A device that refuses the close still has to leave the host's `commit` reporting
/// rather than panicking through its own `Drop` assertion.
#[test]
fn a_refused_close_is_reported_and_still_says_goodbye() {
    let mut device = EmuDevice::new();
    // Answer the close the way a device that has lost the session does. Reached on
    // hardware by resetting the session mid-transaction.
    device.unmodeled().no_session = session::STALE_SESSION;
    let mut t = transport(device);

    let err = block_on(async {
        let mut s = Session::open(&mut t, ObjectClass::Program).await.unwrap();
        // Take the session out from under the host, as a session reset does.
        s.set_read_limit(Duration::from_millis(1));
        let _ = s
            .probe(
                Service::Program,
                10,
                cmd::SESSION_CLOSE,
                &[],
                Duration::from_millis(1),
            )
            .await;
        // The class session is gone now, so anything class-scoped is refused.
        let err = op::info(&mut s, Location::from_user(1, 1))
            .await
            .expect_err("a session-less device refuses");
        s.commit().await.unwrap();
        err
    });
    assert!(
        matches!(err, Error::DeviceStatus(session::STALE_SESSION)),
        "wrong error: {err}"
    );
}
