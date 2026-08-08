//! End-to-end test of the read-only status operation, driven by a replayed exchange.
//!
//! The script below is the real traffic NSM produced for a program-class transaction,
//! taken off the wire. It is protocol framing only — no instrument content — and it
//! makes this a strong test: the code under test has to reproduce a real host's bytes
//! exactly, not merely agree with its own encoder.
//!
//! No hardware, no platform dependency: this runs anywhere the crate compiles,
//! including under Wine, qemu and wasm.

#![cfg(feature = "replay")]

use nord_usb::op;
use nord_usb::transport::{Direction, ReplayTransport, Step};
use nord_usb::wire::ObjectClass;
use nord_usb::Session;

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

/// One complete program-class transaction: open, STATUS, close.
fn program_status_script() -> Vec<Step> {
    use Direction::{In, Out};
    vec![
        // UI/session handshake
        step(Out, "0000001200000006000000010000000006a1"),
        step(In, "000000160000000600000001000000010000000044ec"),
        // SESSION_OPEN, class 4 (programs)
        step(Out, "000000160000000c0000000a0000000400000004a218"),
        step(In, "0000001a0000000c0000000a00000005000000000000000467b0"),
        // STATUS -> count 375, free 3525, used 52875
        step(Out, "000000160000000c0000000a00000008000000042933"),
        step(
            In,
            "0000002a0000000c0000000a0000000900000000000001770000 0dc50000ce8b0000000000000000ac2e"
                .replace(' ', "")
                .as_str(),
        ),
        // SESSION_CLOSE, then the UI side
        step(Out, "000000120000000c0000000a000000066500"),
        step(In, "000000160000000c0000000a00000007000000000c4e"),
        step(Out, "0000001200000006000000010000000226e3"),
        step(In, "0000001600000006000000010000000300000000006f"),
    ]
}

/// Minimal executor — the crate is async but deliberately runtime-agnostic, and a
/// replayed exchange never actually pends, so a busy-poll is sufficient and keeps
/// tokio out of the dependency tree.
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

#[test]
fn status_round_trips_a_real_transaction() {
    let mut t = ReplayTransport::new(program_status_script());

    let got = block_on(async {
        let mut s = Session::open(&mut t, ObjectClass::Program).await.unwrap();
        let status = op::status(&mut s).await.unwrap();
        s.commit().await.unwrap();
        status
    });

    assert_eq!(got.class, ObjectClass::Program);
    assert_eq!(got.count, 375);
    assert_eq!(got.free, 3525);
    assert_eq!(got.used, 52875);
    // free + used is the class capacity; deleting programs was seen to shift the
    // split without changing the total.
    assert_eq!(got.total(), 56400);

    // Every scripted step consumed: the transaction ran to completion, and — because
    // ReplayTransport is Exact by default — every byte sent matched the real host's.
    assert!(t.is_exhausted(), "did not consume the whole exchange");
    assert_eq!(
        t.sent().len(),
        5,
        "expected 5 host messages in this transaction"
    );
}

#[test]
fn wrong_bytes_are_caught() {
    // Opening the wrong class must not silently "work": the bytes differ from the
    // script, so the exact-match transport rejects them.
    let mut t = ReplayTransport::new(program_status_script());
    let err = block_on(async {
        match Session::open(&mut t, ObjectClass::Piano).await {
            Ok(s) => {
                s.abort();
                None
            }
            Err(e) => Some(e),
        }
    });
    assert!(
        err.is_some(),
        "opening the wrong object class should have been rejected"
    );
}

#[test]
fn lenient_mode_tolerates_differing_requests() {
    let mut t = ReplayTransport::new(program_status_script()).lenient();
    let ok = block_on(async {
        let mut s = Session::open(&mut t, ObjectClass::Piano).await?;
        let st = op::status(&mut s).await?;
        s.commit().await?;
        Ok::<_, nord_usb::Error>(st)
    });
    // The replayed response still describes programs; lenient mode is for demos, not
    // for asserting correctness.
    assert_eq!(ok.unwrap().count, 375);
}

/// Fixed-size classes report slots; variable-size ones must not pretend to.
///
/// Numbers are off a real Electro 5: adding one program moved used by exactly 141
/// (53439 -> 53580), and 56400 / 141 is 400 — the instrument's 8 banks x 50 slots.
#[test]
fn derives_slots_only_for_fixed_size_classes() {
    use nord_usb::wire::Status;

    let programs = Status {
        class: ObjectClass::Program,
        count: 380,
        free: 2820,
        used: 53580,
    };
    assert_eq!(programs.blocks_per_item(), Some(141));
    assert_eq!(programs.slots(), Some(400));

    let set_lists = Status {
        class: ObjectClass::SetList,
        count: 63,
        free: 5206,
        used: 2394,
    };
    assert_eq!(set_lists.blocks_per_item(), Some(38));
    assert_eq!(set_lists.slots(), Some(200));

    // Pianos genuinely vary in size, so there is no per-item constant to report.
    let pianos = Status {
        class: ObjectClass::Piano,
        count: 29,
        free: 1,
        used: 4012,
    };
    assert_eq!(pianos.blocks_per_item(), None);
    assert_eq!(pianos.slots(), None);

    // An empty class must not divide by zero.
    let empty = Status {
        class: ObjectClass::Unknown(6),
        count: 0,
        free: 363,
        used: 0,
    };
    assert_eq!(empty.slots(), None);
}

/// Read a program back from the exact traffic NSM produced, and check the result is
/// a byte-perfect `.ne5p`.
///
/// This is the strongest test in the crate: every host message must match NSM's real
/// bytes (the transport is exact-match), and the reconstructed file must equal the
/// `.ne5p` NSM itself saved for that slot.
#[test]
fn read_program_reproduces_nsm_and_rebuilds_the_file() {
    use nord_usb::envelope;
    use Direction::{In, Out};

    // bank 8 slot 14 -> 7, 13 on the wire.
    let script = vec![
        step(Out, "0000001200000006000000010000000006a1"),
        step(In, "000000160000000600000001000000010000000044ec"),
        step(Out, "000000160000000c0000000a0000000400000004a218"),
        step(In, "0000001a0000000c0000000a00000005000000000000000467b0"),
        // INFO
        step(Out, "0000001a0000000c0000000a0000001e000000070000000dc608"),
        step(In, "000000520000000c0000000a0000001f00000000000000070000000d000000796e65357000000004ffffffffffffffff00000010313030303030303030303030303030300000000000000000a5465db65db1"),
        // "Uploading..." progress label the instrument paints — fire-and-forget, no
        // reply. These bytes are straight off the wire; reproducing them is the point.
        step(Out, "000000250000000600000001000000060000000000000c55706c6f6164696e672e2e2ee94e"),
        // BEGIN_READ
        step(Out, "0000001a0000000c0000000a0000000c000000070000000d5391"),
        step(In, "0000001e0000000c0000000a0000000d00000000000000070000000dc4d4"),
        // READ
        step(Out, "000000220000000c0000000a00000012000000070000000d0000000000000079d476"),
        step(In, "0000009f0000000c0000000a0000001300000000000000070000000d0000000000000079000401df06781fc60000000000000000000000000000000000000100000000000000000000400000000000000002200000000000022000400000008888000008008888000008000000000080000000000080000000000000000000800000000800800000000800020010060401020408140010000000000000d24c"),
        // 100% progress bar, also fire-and-forget.
        step(Out, "0000001600000006000000010000000700010064927b"),
        // END_TRANSFER
        step(Out, "0000001a0000000c0000000a0000000e000000070000000d95f6"),
        step(In, "0000001e0000000c0000000a0000000f00000000000000070000000d4e12"),
        // close
        step(Out, "000000120000000c0000000a000000066500"),
        step(In, "000000160000000c0000000a00000007000000000c4e"),
        step(Out, "0000001200000006000000010000000226e3"),
        step(In, "0000001600000006000000010000000300000000006f"),
    ];

    let at = nord_usb::Location::from_user(8, 14);
    let mut t = ReplayTransport::new(script);
    let file = block_on(async {
        let mut s = Session::open(&mut t, ObjectClass::Program).await.unwrap();
        let f = match op::read_program(&mut s, at).await {
            Ok(f) => f,
            Err(e) => {
                s.abort();
                panic!("read_program failed: {e}")
            }
        };
        s.commit().await.unwrap();
        f
    });

    // The real 165-byte .ne5p NSM saved for this slot.
    let expected = hex("4342494e010000006e65357007000d00ffffffff04000000b65d46a500000000000000000000000000000000000401df06781fc60000000000000000000000000000000000000100000000000000000000400000000000000002200000000000022000400000008888000008008888000008000000000080000000000080000000000000000000800000000800800000000800020010060401020408140010000000000000");
    assert_eq!(
        file, expected,
        "reconstructed .ne5p differs from the file NSM saved"
    );

    // And it survives a trip back out to the wire body.
    let back = envelope::unwrap(&file).unwrap();
    assert_eq!(envelope::tag(&back.header), "ne5p");
    assert_eq!(envelope::location(&back.header), at);
    assert_eq!(back.body.0.len(), 121);

    assert!(t.is_exhausted(), "did not consume the whole exchange");
}

/// A body larger than one `READ` arrives across several requests, and the offsets must
/// advance by exactly what was asked for.
///
/// The framing here is built rather than captured — the captured-bytes test above already
/// pins that. What this pins is the chunking: three exchanges at offsets 0 / 32720 / 65440
/// with lengths 32720 / 32720 / 777, in that order, under an exact-match transport. A
/// single whole-body request, a wrong offset, or a dropped final chunk all fail it.
#[test]
fn a_large_body_is_read_in_chunks() {
    use nord_usb::wire::{cmd, Message, Service};
    use Direction::{In, Out};

    const CHUNK: u32 = 32720;
    const TAIL: u32 = 777;
    let body_len = CHUNK * 2 + TAIL;

    // Position-dependent, so chunks reassembled out of order or with a gap are caught.
    let body: Vec<u8> = (0..body_len).map(|i| (i % 251) as u8).collect();

    // bank 8 slot 14 -> 7, 13 on the wire.
    let (bank, slot) = (7u32, 13u32);
    let loc = |v: &mut Vec<u8>| {
        v.extend_from_slice(&bank.to_be_bytes());
        v.extend_from_slice(&slot.to_be_bytes());
    };
    let response = |command: u32, rest: &[u8]| {
        let mut args = vec![0, 0, 0, 0]; // status 0
        args.extend_from_slice(rest);
        Message::new(Service::Program, 10, command, args).encode()
    };

    let mut info_args = Vec::new();
    loc(&mut info_args);
    info_args.extend_from_slice(&body_len.to_be_bytes());
    info_args.extend_from_slice(b"ne5p");
    info_args.extend_from_slice(&4u32.to_be_bytes()); // version
    info_args.extend_from_slice(&u32::MAX.to_be_bytes());
    info_args.extend_from_slice(&u32::MAX.to_be_bytes());
    info_args.extend_from_slice(&8u32.to_be_bytes()); // name length
    info_args.extend_from_slice(b"chunked ");
    info_args.extend_from_slice(&0u32.to_be_bytes()); // crc32: none

    let mut script = vec![
        step(Out, "0000001200000006000000010000000006a1"),
        step(In, "000000160000000600000001000000010000000044ec"),
        step(Out, "000000160000000c0000000a0000000400000004a218"),
        step(In, "0000001a0000000c0000000a00000005000000000000000467b0"),
        step(Out, "0000001a0000000c0000000a0000001e000000070000000dc608"),
        Step {
            direction: In,
            bytes: response(cmd::INFO + 1, &info_args),
        },
        step(
            Out,
            "000000250000000600000001000000060000000000000c55706c6f6164696e672e2e2ee94e",
        ),
        step(Out, "0000001a0000000c0000000a0000000c000000070000000d5391"),
        step(
            In,
            "0000001e0000000c0000000a0000000d00000000000000070000000dc4d4",
        ),
    ];

    // The bar after each chunk: 32720/66217 = 49.4%, 65440/66217 = 98.8%, then done.
    // Written out rather than recomputed, so a wrong formula fails instead of agreeing
    // with itself.
    for (offset, want, pct) in [
        (0, CHUNK, 49u16),
        (CHUNK, CHUNK, 98),
        (CHUNK * 2, TAIL, 100),
    ] {
        let mut req = Vec::new();
        loc(&mut req);
        req.extend_from_slice(&offset.to_be_bytes());
        req.extend_from_slice(&want.to_be_bytes());
        script.push(Step {
            direction: Out,
            bytes: Message::new(Service::Program, 10, cmd::READ, req.clone()).encode(),
        });

        let mut resp = req.clone();
        resp.extend_from_slice(&body[offset as usize..(offset + want) as usize]);
        script.push(Step {
            direction: In,
            bytes: response(cmd::READ + 1, &resp),
        });
        script.push(Step {
            direction: Out,
            bytes: nord_usb::wire::ui::percent(pct).encode(),
        });
    }

    script.extend([
        step(Out, "0000001a0000000c0000000a0000000e000000070000000d95f6"),
        step(
            In,
            "0000001e0000000c0000000a0000000f00000000000000070000000d4e12",
        ),
        step(Out, "000000120000000c0000000a000000066500"),
        step(In, "000000160000000c0000000a00000007000000000c4e"),
        step(Out, "0000001200000006000000010000000226e3"),
        step(In, "0000001600000006000000010000000300000000006f"),
    ]);

    let at = nord_usb::Location::from_user(8, 14);
    let mut t = ReplayTransport::new(script);
    let got = block_on(async {
        let mut s = Session::open(&mut t, ObjectClass::Program).await.unwrap();
        let r = match op::read_body(&mut s, at).await {
            Ok(b) => b,
            Err(e) => {
                s.abort();
                panic!("read_body failed: {e}")
            }
        };
        s.commit().await.unwrap();
        r
    });

    assert_eq!(
        got.len(),
        body_len as usize,
        "reassembled body is the wrong length"
    );
    assert_eq!(
        got, body,
        "reassembled body differs from what the device sent"
    );
    assert!(t.is_exhausted(), "did not consume the whole exchange");
}
