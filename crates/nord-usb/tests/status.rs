//! What the read path decodes, and how strictly a replay polices what is sent.
//!
//! The exchanges live in `tests/scripts` and are replayed by `tests/replay`, which
//! checks the bytes; these are the assertions about what those bytes *mean* — the
//! counters a `STATUS` reply carries, the container a read rebuilds, the chunking a body
//! larger than one request goes through — plus the two properties of the replay
//! transport itself that only a deliberately wrong caller can show.
//!
//! No hardware, no platform dependency: this runs anywhere the crate compiles,
//! including under Wine, qemu and wasm.

#![cfg(feature = "replay")]

#[path = "support/scripts.rs"]
mod scripts;

use nord_usb::op;
use nord_usb::transport::{Direction, ReplayTransport, Step};
use nord_usb::wire::ObjectClass;
use nord_usb::Session;

/// The program-class transaction NSM produced, as the sweep drives it.
fn program_status() -> ReplayTransport {
    ReplayTransport::new(scripts::fixture("program/status_program.script").steps())
}

#[test]
fn status_decodes_the_counters_a_real_transaction_carried() {
    let mut t = program_status();
    let got = pollster::block_on(async {
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
    let mut t = program_status();
    let err = pollster::block_on(async {
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
    let mut t = program_status().lenient();
    let ok = pollster::block_on(async {
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

/// The file a read rebuilds is a real `.ne5p`, not just the right bytes.
///
/// `tests/scripts/program/read_prog_bank8_loc14.script` pins the reconstruction against
/// the file NSM saved for that slot, byte for byte; this says what those bytes are —
/// a container whose header carries the format tag and the address the wire never
/// transmits together.
#[test]
fn a_rebuilt_file_is_a_container_the_envelope_reads_back() {
    use nord_usb::envelope;

    let at = nord_usb::Location::from_user(8, 14);
    let file = std::fs::read(scripts::fixtures().join("program/prog_8-14.ne5p")).unwrap();
    let back = envelope::unwrap(&file).unwrap();
    assert_eq!(envelope::tag(&back.header), "ne5p");
    assert_eq!(envelope::location(&back.header), at);
    assert_eq!(back.body.0.len(), 121);
}

/// A body larger than one `READ` arrives across several requests, and the offsets must
/// advance by exactly what was asked for.
///
/// The framing is built rather than captured — `read_prog_bank8_loc14.script` already
/// pins the captured bytes. What this pins is the chunking: three exchanges at offsets
/// 0 / 32720 / 65440 with lengths 32720 / 32720 / 777, in that order, under an
/// exact-match transport. A single whole-body request, a wrong offset, or a dropped
/// final chunk all fail it.
#[test]
fn a_large_body_is_read_in_chunks() {
    use nord_usb::wire::{cmd, ui, Message, Service};
    use Direction::{In, Out};

    const CHUNK: u32 = 32720;
    const TAIL: u32 = 777;
    let body_len = CHUNK * 2 + TAIL;

    // Position-dependent, so chunks reassembled out of order or with a gap are caught.
    let body: Vec<u8> = (0..body_len).map(|i| (i % 251) as u8).collect();

    // bank 8 slot 14 -> 7, 13 on the wire.
    let at = nord_usb::Location::from_user(8, 14);
    let mut slot = Vec::new();
    at.write_to(&mut slot);

    let request = |command: u32, args: &[u8]| Step {
        direction: Out,
        bytes: Message::new(Service::Program, 10, command, args.to_vec()).encode(),
    };
    let response = |command: u32, rest: &[u8]| Step {
        direction: In,
        bytes: Message::new(
            Service::Program,
            10,
            command,
            [&0u32.to_be_bytes()[..], rest].concat(),
        )
        .encode(),
    };
    let notify = |msg: Message| Step {
        direction: Out,
        bytes: msg.encode(),
    };
    let ui_frame = |command: u32, args: &[u8]| Step {
        direction: Out,
        bytes: Message::new(Service::Ui, ui::SUBSYSTEM, command, args.to_vec()).encode(),
    };

    let mut info_args = slot.clone();
    info_args.extend_from_slice(&body_len.to_be_bytes());
    info_args.extend_from_slice(b"ne5p");
    info_args.extend_from_slice(&4u32.to_be_bytes()); // version
    info_args.extend_from_slice(&u32::MAX.to_be_bytes());
    info_args.extend_from_slice(&u32::MAX.to_be_bytes());
    info_args.extend_from_slice(&8u32.to_be_bytes()); // name length
    info_args.extend_from_slice(b"chunked ");
    info_args.extend_from_slice(&0u32.to_be_bytes()); // crc32: none

    let mut script = vec![
        ui_frame(ui::HELLO, &[]),
        Step {
            direction: In,
            bytes: Message::new(Service::Ui, ui::SUBSYSTEM, ui::HELLO + 1, vec![0; 4]).encode(),
        },
        request(
            cmd::SESSION_OPEN,
            &ObjectClass::Program.to_raw().to_be_bytes(),
        ),
        response(
            cmd::SESSION_OPEN + 1,
            &ObjectClass::Program.to_raw().to_be_bytes(),
        ),
        request(cmd::INFO, &slot),
        response(cmd::INFO + 1, &info_args),
        notify(ui::label("Uploading...").unwrap()),
        request(cmd::BEGIN_READ, &slot),
        response(cmd::BEGIN_READ + 1, &slot),
    ];

    // The bar after each chunk: 32720/66217 = 49.4%, 65440/66217 = 98.8%, then done.
    // Written out rather than recomputed, so a wrong formula fails instead of agreeing
    // with itself.
    for (offset, want, pct) in [
        (0, CHUNK, 49u16),
        (CHUNK, CHUNK, 98),
        (CHUNK * 2, TAIL, 100),
    ] {
        let mut req = slot.clone();
        req.extend_from_slice(&offset.to_be_bytes());
        req.extend_from_slice(&want.to_be_bytes());
        script.push(request(cmd::READ, &req));

        let mut resp = req.clone();
        resp.extend_from_slice(&body[offset as usize..(offset + want) as usize]);
        script.push(response(cmd::READ + 1, &resp));
        script.push(notify(ui::percent(pct)));
    }

    script.extend([
        request(cmd::END_TRANSFER, &slot),
        response(cmd::END_TRANSFER + 1, &slot),
        request(cmd::SESSION_CLOSE, &[]),
        response(cmd::SESSION_CLOSE + 1, &[]),
        ui_frame(ui::GOODBYE, &[]),
        Step {
            direction: In,
            bytes: Message::new(Service::Ui, ui::SUBSYSTEM, ui::GOODBYE + 1, vec![0; 4]).encode(),
        },
    ]);

    let mut t = ReplayTransport::new(script);
    let got = pollster::block_on(async {
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
