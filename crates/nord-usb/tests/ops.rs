//! Golden tests for the mutating and select operations, each driven by a replayed
//! exchange taken straight off the wire from a real NSM capture.
//!
//! The transport is exact-match, so every host message the operation emits has to equal
//! the bytes NSM sent for that same action — not merely agree with our own encoder. The
//! trailing UI-refresh reads NSM issues to repaint its browser (see `op`'s module note)
//! are omitted from both the implementation and these scripts.
//!
//! No hardware, no platform dependency: runs anywhere the crate compiles.

#![cfg(feature = "replay")]

use nord_usb::op;
use nord_usb::transport::{Direction, ReplayTransport, Step};
use nord_usb::wire::ObjectClass;
use nord_usb::{Location, Session};

fn hex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

fn step(d: Direction, s: &str) -> Step {
    Step {
        direction: d,
        bytes: hex(s),
    }
}

/// The session open/close wrapper every program-class transaction shares, with `middle`
/// spliced in — the actual per-op exchange.
fn wrap(middle: Vec<Step>) -> Vec<Step> {
    use Direction::{In, Out};
    let mut v = vec![
        step(Out, "0000001200000006000000010000000006a1"),
        step(In, "000000160000000600000001000000010000000044ec"),
        step(Out, "000000160000000c0000000a0000000400000004a218"),
        step(In, "0000001a0000000c0000000a00000005000000000000000467b0"),
    ];
    v.extend(middle);
    v.extend([
        step(Out, "000000120000000c0000000a000000066500"),
        step(In, "000000160000000c0000000a00000007000000000c4e"),
        step(Out, "0000001200000006000000010000000226e3"),
        step(In, "0000001600000006000000010000000300000000006f"),
    ]);
    v
}

/// Minimal executor — the replayed exchange never actually pends, so a busy-poll keeps
/// tokio out of the dependency tree. Mirrors the one in `status.rs`.
fn block_on<F: std::future::Future>(mut fut: F) -> F::Output {
    use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
    fn vtable() -> &'static RawWakerVTable {
        &RawWakerVTable::new(
            |_| RawWaker::new(std::ptr::null(), vtable()),
            |_| {},
            |_| {},
            |_| {},
        )
    }
    let waker = unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), vtable())) };
    let mut cx = Context::from_waker(&waker);
    let mut fut = unsafe { std::pin::Pin::new_unchecked(&mut fut) };
    loop {
        if let Poll::Ready(v) = fut.as_mut().poll(&mut cx) {
            return v;
        }
    }
}

/// A close that the device refuses must surface as the `Err` it is — and must still
/// release the UI session.
///
/// `commit()` consumes the session, so a failure inside it drops the session mid-close;
/// if `closed` were only marked on success, the debug `Drop` assertion would panic over
/// the very error the caller was owed — in exactly the always-close error paths the CLI
/// is built around. And the `HELLO` is the half that wedges the instrument, so the
/// refused `SESSION_CLOSE` must not skip the `GOODBYE`: the script ends with that
/// exchange, so `is_exhausted` is the assertion that it was sent. The golden replays
/// never fail a close, so only this test reaches either property.
#[test]
fn a_failed_commit_reports_rather_than_panicking() {
    use Direction::{In, Out};
    let mut t = ReplayTransport::new(vec![
        step(Out, "0000001200000006000000010000000006a1"),
        step(In, "000000160000000600000001000000010000000044ec"),
        step(Out, "000000160000000c0000000a0000000400000004a218"),
        step(In, "0000001a0000000c0000000a00000005000000000000000467b0"),
        step(Out, "000000120000000c0000000a000000066500"),
        // SESSION_CLOSE answered with device status 5 instead of success. Framed by the
        // codec so the CRC is right; the decoder marks direction, not the bytes.
        Step {
            direction: In,
            bytes: nord_usb::Message::new(
                nord_usb::Service::Program,
                10,
                0x07,
                5u32.to_be_bytes().to_vec(),
            )
            .encode(),
        },
        step(Out, "0000001200000006000000010000000226e3"),
        step(In, "0000001600000006000000010000000300000000006f"),
    ]);
    let err = block_on(async {
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

/// A `SESSION_OPEN` the device refuses must still release the UI session the preceding
/// `HELLO` opened.
///
/// Left half-open the instrument keeps answering and reports every slot in every class as
/// empty — a wrong answer that looks like a right one, surviving reopening and clearing
/// only on a power cycle. The script ends with the `GOODBYE` exchange, so `is_exhausted`
/// is the assertion that it was sent.
#[test]
fn a_refused_open_still_says_goodbye() {
    use Direction::{In, Out};
    let mut t = ReplayTransport::new(vec![
        step(Out, "0000001200000006000000010000000006a1"),
        step(In, "000000160000000600000001000000010000000044ec"),
        step(Out, "000000160000000c0000000a0000000400000004a218"),
        // SESSION_OPEN answered with device status 5 instead of success.
        Step {
            direction: In,
            bytes: nord_usb::Message::new(
                nord_usb::Service::Program,
                10,
                0x05,
                5u32.to_be_bytes().to_vec(),
            )
            .encode(),
        },
        step(Out, "0000001200000006000000010000000226e3"),
        step(In, "0000001600000006000000010000000300000000006f"),
    ]);
    // `Session` is deliberately not `Debug`, so this is `expect_err` by hand. The `abort`
    // keeps the failing case from panicking a second time inside `Drop` during unwind.
    let err = block_on(async {
        match Session::open(&mut t, ObjectClass::Program).await {
            Ok(s) => {
                s.abort();
                panic!("the device refused the open, but it was reported as success");
            }
            Err(e) => e,
        }
    });
    assert!(
        matches!(err, nord_usb::Error::DeviceStatus(5)),
        "wrong error: {err}"
    );
    assert!(
        t.is_exhausted(),
        "the refused open did not send GOODBYE, leaving the device half-open"
    );
}

/// A `HELLO` whose reply is unusable must still release the session best-effort.
///
/// The write landed, so the device may already be holding the UI session even though it
/// answered with a refusal — and a held UI session is the wedge described on
/// [`a_refused_open_still_says_goodbye`]. Same script shape: it ends with the `GOODBYE`
/// exchange, so `is_exhausted` is the assertion that it was sent.
#[test]
fn a_refused_hello_still_says_goodbye() {
    use Direction::{In, Out};
    let mut t = ReplayTransport::new(vec![
        step(Out, "0000001200000006000000010000000006a1"),
        // HELLO answered with device status 5 instead of success.
        Step {
            direction: In,
            bytes: nord_usb::Message::new(
                nord_usb::Service::Ui,
                1,
                0x01,
                5u32.to_be_bytes().to_vec(),
            )
            .encode(),
        },
        step(Out, "0000001200000006000000010000000226e3"),
        step(In, "0000001600000006000000010000000300000000006f"),
    ]);
    let err = block_on(async {
        match Session::open(&mut t, ObjectClass::Program).await {
            Ok(s) => {
                s.abort();
                panic!("the device refused the HELLO, but it was reported as success");
            }
            Err(e) => e,
        }
    });
    assert!(
        matches!(err, nord_usb::Error::DeviceStatus(5)),
        "wrong error: {err}"
    );
    assert!(
        t.is_exhausted(),
        "the refused HELLO did not attempt GOODBYE"
    );
}

/// An unsolicited [`cmd::CHANGED`] notification, framed by the codec. The device
/// queues one on its own (a front-panel STORE, observed on hardware), and the next
/// host read receives it in place of the reply it was waiting for.
fn changed_notification() -> Step {
    Step {
        direction: Direction::In,
        bytes: nord_usb::Message::new(
            nord_usb::Service::Program,
            10,
            nord_usb::wire::cmd::CHANGED,
            Vec::new(),
        )
        .encode(),
    }
}

/// A queued notification must be drained — the real reply read out from behind it —
/// and surfaced, not mistaken for the reply. Mistaking it failed the open *and*
/// wedged the instrument on hardware: the mismatch bailed without the drain, and the
/// session never recovered.
#[test]
fn an_unsolicited_changed_notification_is_drained_not_mistaken_for_the_reply() {
    use Direction::{In, Out};
    let mut t = ReplayTransport::new(vec![
        step(Out, "0000001200000006000000010000000006a1"),
        changed_notification(),
        step(In, "000000160000000600000001000000010000000044ec"),
        step(Out, "000000160000000c0000000a0000000400000004a218"),
        step(In, "0000001a0000000c0000000a00000005000000000000000467b0"),
        step(Out, "000000120000000c0000000a000000066500"),
        step(In, "000000160000000c0000000a00000007000000000c4e"),
        step(Out, "0000001200000006000000010000000226e3"),
        step(In, "0000001600000006000000010000000300000000006f"),
    ]);
    block_on(async {
        let s = Session::open(&mut t, ObjectClass::Program).await.unwrap();
        assert!(
            s.instrument_changed(),
            "the drained notification must be surfaced, not silently skipped"
        );
        s.commit().await.unwrap();
    });
    assert!(t.is_exhausted(), "did not consume the whole exchange");
}

/// A mid-session reply answering the wrong command is a desync: nothing read after it
/// can be paired with its request, so the transaction is over. The bail must still
/// say GOODBYE — the HELLO is the half that wedges the instrument — and the
/// always-commit discipline callers follow must stay off the wire afterwards, which
/// the exhausted script asserts: any traffic from the `commit` would fail it.
#[test]
fn a_mid_session_unexpected_response_still_says_goodbye() {
    use Direction::{In, Out};
    let mut t = ReplayTransport::new(vec![
        step(Out, "0000001200000006000000010000000006a1"),
        step(In, "000000160000000600000001000000010000000044ec"),
        step(Out, "000000160000000c0000000a0000000400000004a218"),
        step(In, "0000001a0000000c0000000a00000005000000000000000467b0"),
        // SELECT 2:12, answered with an INFO reply (0x1f) instead of its 0x30.
        step(Out, "0000001a0000000c0000000a0000002f000000010000000b746a"),
        Step {
            direction: In,
            bytes: nord_usb::Message::new(
                nord_usb::Service::Program,
                10,
                0x1f,
                0u32.to_be_bytes().to_vec(),
            )
            .encode(),
        },
        step(Out, "0000001200000006000000010000000226e3"),
        step(In, "0000001600000006000000010000000300000000006f"),
    ]);
    let err = block_on(async {
        let mut s = Session::open(&mut t, ObjectClass::Program).await.unwrap();
        let err = op::select(&mut s, Location::from_user(2, 12))
            .await
            .expect_err("the reply answered the wrong command");
        s.commit().await.unwrap();
        err
    });
    assert!(
        matches!(
            err,
            nord_usb::Error::UnexpectedResponse {
                expected: 0x30,
                got: 0x1f
            }
        ),
        "wrong error: {err}"
    );
    assert!(
        t.is_exhausted(),
        "the bail did not send GOODBYE, leaving the device half-open"
    );
}

/// The drain is capped: a device streaming notifications must not pin the host in the
/// read loop forever. Past [`nord_usb::session::DRAIN_CAP`] the notification is
/// reported as the unexpected response it is — and that bail still says GOODBYE.
#[test]
fn a_notification_flood_bails_rather_than_looping() {
    use Direction::{In, Out};
    let mut script = vec![step(Out, "0000001200000006000000010000000006a1")];
    script.extend(vec![
        changed_notification();
        nord_usb::session::DRAIN_CAP + 1
    ]);
    script.push(step(Out, "0000001200000006000000010000000226e3"));
    script.push(step(In, "0000001600000006000000010000000300000000006f"));
    let mut t = ReplayTransport::new(script);
    let err = block_on(async {
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

/// `move_prog_8-13_to_7-16`: a single MOVE with two addresses in one small op.
#[test]
fn move_reproduces_the_capture() {
    use Direction::{In, Out};
    let mut t = ReplayTransport::new(wrap(vec![
        step(
            Out,
            "000000220000000c0000000a00000018000000070000000c000000060000000f4a55",
        ),
        step(
            In,
            "000000260000000c0000000a0000001900000000000000070000000c000000060000000f7197",
        ),
    ]));
    block_on(async {
        let mut s = Session::open(&mut t, ObjectClass::Program)
            .await
            .unwrap()
            .allow_destructive_writes();
        op::move_object(
            &mut s,
            Location::from_user(8, 13),
            Location::from_user(7, 16),
        )
        .await
        .unwrap();
        s.commit().await.unwrap();
    });
    assert!(t.is_exhausted(), "did not consume the whole exchange");
}

/// `delete_prog_bank7_loc50`: the `"Deleting..."` label (fire-and-forget) then DELETE.
#[test]
fn delete_reproduces_the_capture() {
    use Direction::{In, Out};
    let mut t = ReplayTransport::new(wrap(vec![
        step(
            Out,
            "000000240000000600000001000000060000000000000b44656c6574696e672e2e2e7394",
        ),
        step(Out, "0000001a0000000c0000000a000000140000000600000031741e"),
        step(
            In,
            "0000001e0000000c0000000a0000001500000000000000060000003184b4",
        ),
    ]));
    block_on(async {
        let mut s = Session::open(&mut t, ObjectClass::Program)
            .await
            .unwrap()
            .allow_destructive_writes();
        op::delete(&mut s, Location::from_user(7, 50))
            .await
            .unwrap();
        s.commit().await.unwrap();
    });
    assert!(t.is_exhausted(), "did not consume the whole exchange");
}

/// `rename_prog_6-13`: RENAME carrying the length-prefixed `"foo"`.
#[test]
fn rename_reproduces_the_capture() {
    use Direction::{In, Out};
    let mut t = ReplayTransport::new(wrap(vec![
        step(
            Out,
            "000000210000000c0000000a0000001c000000050000000c00000003666f6f0d53",
        ),
        step(
            In,
            "0000001e0000000c0000000a0000001d00000000000000050000000c86c2",
        ),
    ]));
    block_on(async {
        let mut s = Session::open(&mut t, ObjectClass::Program)
            .await
            .unwrap()
            .allow_destructive_writes();
        op::rename(&mut s, Location::from_user(6, 13), "foo")
            .await
            .unwrap();
        s.commit().await.unwrap();
    });
    assert!(t.is_exhausted(), "did not consume the whole exchange");
}

/// `duplicate_prog_7-2_to_7-3`: one COPY with source and destination.
#[test]
fn duplicate_reproduces_the_capture() {
    use Direction::{In, Out};
    let mut t = ReplayTransport::new(wrap(vec![
        step(
            Out,
            "000000220000000c0000000a000000160000000600000001000000060000000265f4",
        ),
        step(
            In,
            "000000260000000c0000000a000000170000000000000006000000010000000600000002a86a",
        ),
    ]));
    block_on(async {
        let mut s = Session::open(&mut t, ObjectClass::Program)
            .await
            .unwrap()
            .allow_destructive_writes();
        op::duplicate(&mut s, Location::from_user(7, 2), Location::from_user(7, 3))
            .await
            .unwrap();
        s.commit().await.unwrap();
    });
    assert!(t.is_exhausted(), "did not consume the whole exchange");
}

/// `open_on_device_2-12`: SELECT (`0x2f` → `0x30`, the inverted-parity command).
/// Non-destructive, so a plain read-only session reaches it.
#[test]
fn select_reproduces_the_capture() {
    use Direction::{In, Out};
    let mut t = ReplayTransport::new(wrap(vec![
        step(Out, "0000001a0000000c0000000a0000002f000000010000000b746a"),
        step(
            In,
            "0000001e0000000c0000000a0000003000000000000000010000000b19df",
        ),
    ]));
    block_on(async {
        let mut s = Session::open(&mut t, ObjectClass::Program).await.unwrap();
        op::select(&mut s, Location::from_user(2, 12))
            .await
            .unwrap();
        s.commit().await.unwrap();
    });
    assert!(t.is_exhausted(), "did not consume the whole exchange");
}

/// The dependency read from `duplicate_prog_7-2_to_7-3`: a piano and a sample, each
/// carrying the content id that also appears in the file header.
#[test]
fn dependencies_decode_the_capture() {
    use Direction::{In, Out};
    let mut t = ReplayTransport::new(wrap(vec![
        step(Out, "0000001a0000000c0000000a000000280000000600000002333c"),
        step(In, "000000820000000c0000000a0000002900000000000000060000000200000002000000000000000001d303b5f20000001a526f79616c204772616e64203344205961533620584c20352e3400000000ffffffffffffffff010000000000000003f2f5cadc0000000c6166726963615f73706c697400000000ffffffffffffffffc791"),
    ]));
    let deps = block_on(async {
        let mut s = Session::open(&mut t, ObjectClass::Program).await.unwrap();
        let deps = op::dependencies(&mut s, Location::from_user(7, 3))
            .await
            .unwrap();
        s.commit().await.unwrap();
        deps
    });
    assert_eq!(deps.len(), 2);
    assert_eq!(deps[0].class, ObjectClass::Piano);
    assert_eq!(deps[0].name, "Royal Grand 3D YaS6 XL 5.4");
    assert_eq!(deps[1].class, ObjectClass::Sample);
    assert_eq!(deps[1].name, "africa_split");
    assert!(t.is_exhausted(), "did not consume the whole exchange");
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
    use Direction::{In, Out};

    fn program(command: u32, status: u32) -> Step {
        Step {
            direction: In,
            bytes: nord_usb::Message::new(
                nord_usb::Service::Program,
                10,
                command,
                status.to_be_bytes().to_vec(),
            )
            .encode(),
        }
    }

    let mut t = ReplayTransport::new(vec![
        step(Out, "0000001200000006000000010000000006a1"),
        step(In, "000000160000000600000001000000010000000044ec"),
        // The open the device refuses because it is still holding one.
        step(Out, "000000160000000c0000000a0000000400000004a218"),
        program(0x05, 0x12),
        // The recovery: SESSION_CLOSE with nothing wrapped around it.
        step(Out, "000000120000000c0000000a000000066500"),
        program(0x07, 0),
        // ...and the same open again, now accepted.
        step(Out, "000000160000000c0000000a0000000400000004a218"),
        step(In, "0000001a0000000c0000000a00000005000000000000000467b0"),
        // Ordinary close.
        step(Out, "000000120000000c0000000a000000066500"),
        step(In, "000000160000000c0000000a00000007000000000c4e"),
        step(Out, "0000001200000006000000010000000226e3"),
        step(In, "0000001600000006000000010000000300000000006f"),
    ]);

    block_on(async {
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

/// The retry happens once, not in a loop.
///
/// A device that answers `0x12` to the retry as well is genuinely broken, and the caller
/// is owed that error rather than a hang.
#[test]
fn a_stale_session_that_will_not_clear_is_reported() {
    use Direction::{In, Out};

    fn refused(command: u32) -> Step {
        Step {
            direction: In,
            bytes: nord_usb::Message::new(
                nord_usb::Service::Program,
                10,
                command,
                0x12u32.to_be_bytes().to_vec(),
            )
            .encode(),
        }
    }

    let mut t = ReplayTransport::new(vec![
        step(Out, "0000001200000006000000010000000006a1"),
        step(In, "000000160000000600000001000000010000000044ec"),
        step(Out, "000000160000000c0000000a0000000400000004a218"),
        refused(0x05),
        step(Out, "000000120000000c0000000a000000066500"),
        refused(0x07),
        // Refused again: the retry happens once, not in a loop.
        step(Out, "000000160000000c0000000a0000000400000004a218"),
        refused(0x05),
        // The failed open still releases the UI session.
        step(Out, "0000001200000006000000010000000226e3"),
        step(In, "0000001600000006000000010000000300000000006f"),
    ]);

    let err = block_on(async {
        match Session::open(&mut t, ObjectClass::Program).await {
            Ok(s) => {
                s.abort();
                panic!("the device never accepted the open, but it was reported as success");
            }
            Err(e) => e,
        }
    });
    assert!(
        matches!(err, nord_usb::Error::DeviceStatus(0x12)),
        "wrong error: {err}"
    );
    assert!(
        t.is_exhausted(),
        "a session that would not clear did not say GOODBYE"
    );
}

/// A walk the device refuses mid-way must surface the refusal, not a partial list.
///
/// `0x11` is the instrument disabling enumeration, which it does after any write since
/// power-up ([`op::ENUMERATION_DISABLED`]). The dangerous failure mode would be
/// `occupied_slots` treating the refusal like the end-of-walk status and returning
/// whatever it had — an inventory that looks complete. No golden capture exists for this
/// exchange (NSM never sends `NEXT_SLOT`), so the requests are built with our own
/// encoder rather than replayed. The close still runs on the error path; the script
/// ending with those exchanges makes `is_exhausted` the assertion that it was sent.
#[test]
fn a_disabled_cursor_is_an_error_not_a_partial_list() {
    use nord_usb::{Message, Service};
    use Direction::{In, Out};

    let msg = |cmd: u32, args: Vec<u8>| Message::new(Service::Program, 10, cmd, args).encode();
    let middle = vec![
        // INFO 0:0 answered "empty" — a refusal the walk tolerates by design.
        Step {
            direction: Out,
            bytes: msg(0x1e, vec![0; 8]),
        },
        Step {
            direction: In,
            bytes: msg(0x1f, 1u32.to_be_bytes().to_vec()),
        },
        // NEXT_SLOT 0:0 answered with enumeration disabled.
        Step {
            direction: Out,
            bytes: msg(0x20, vec![0; 8]),
        },
        Step {
            direction: In,
            bytes: msg(0x21, 0x11u32.to_be_bytes().to_vec()),
        },
    ];
    let mut t = ReplayTransport::new(wrap(middle));

    let err = block_on(async {
        let mut s = Session::open(&mut t, ObjectClass::Program).await.unwrap();
        let r = op::occupied_slots(&mut s, 500).await;
        s.commit()
            .await
            .expect("the close itself is not refused here");
        r.expect_err("the refused walk was reported as success")
    });
    assert!(
        matches!(err, nord_usb::Error::DeviceStatus(op::ENUMERATION_DISABLED)),
        "wrong error: {err}"
    );
    assert!(
        t.is_exhausted(),
        "the refused walk did not run the closing exchanges"
    );
}
