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

/// A close that the device refuses must surface as the `Err` it is.
///
/// `commit()` consumes the session, so a failure inside it drops the session mid-close;
/// if `closed` were only marked on success, the debug `Drop` assertion would panic over
/// the very error the caller was owed — in exactly the always-close error paths the CLI
/// is built around. The golden replays never fail a close, so only this test reaches it.
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
    ]);
    let err = block_on(async {
        let s = Session::open(&mut t, ObjectClass::Program).await.unwrap();
        s.commit().await.expect_err("the device refused the close")
    });
    assert!(
        matches!(err, nord_usb::Error::DeviceStatus(5)),
        "wrong error: {err}"
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
