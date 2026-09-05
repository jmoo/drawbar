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
    // A slot class parks nothing: deleting a program returns its bytes to `free`
    // directly, so both trailing words stay zero here.
    assert_eq!(got.dirty, 0);
    assert_eq!(got.spare, 0);
    assert_eq!(got.total(), 56400);
    assert_eq!(got.available(), 3525);

    assert!(t.is_exhausted(), "did not consume the whole exchange");
    assert_eq!(
        t.sent().len(),
        5,
        "expected 5 host messages in this transaction"
    );
}

/// A `STATUS` response frame carrying `payload` after the success status word.
fn status_reply(payload: &[u8]) -> nord_usb::wire::Message {
    use nord_usb::wire::{cmd, Message, Service};

    let args = [&0u32.to_be_bytes()[..], payload].concat();
    Message::decode_response(&Message::new(Service::Program, 10, cmd::STATUS + 1, args).encode())
        .unwrap()
}

fn counters(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|w| w.to_be_bytes()).collect()
}

/// A reply that stops after the three counters every class reports still decodes; the
/// words it does not carry read zero rather than failing the decode.
#[test]
fn a_three_word_status_reply_reads_no_dirty_or_spare() {
    use nord_usb::wire::Status;

    let got = Status::decode(
        ObjectClass::Program,
        &status_reply(&counters(&[375, 3525, 52875])),
    )
    .unwrap();

    assert_eq!(got.count, 375);
    assert_eq!(got.free, 3525);
    assert_eq!(got.used, 52875);
    assert_eq!(got.dirty, 0);
    assert_eq!(got.spare, 0);
    assert_eq!(got.total(), 56400);
}

/// One byte short of three words there is no whole `used` to report, so the reply is
/// refused rather than decoded around a counter cut in half.
#[test]
fn a_status_reply_short_of_three_words_is_refused() {
    use nord_usb::wire::Status;

    let mut payload = counters(&[375, 3525, 52875]);
    payload.truncate(11);
    let err = Status::decode(ObjectClass::Program, &status_reply(&payload))
        .expect_err("11 bytes is not a decodable STATUS payload");

    assert!(
        matches!(err, nord_usb::Error::Truncated { got: 11, need: 12 }),
        "{err}"
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
/// (53439 -> 53580) — 121 body + 16 name + 4 CRC — and 56400 / 141 is 400, the
/// instrument's 8 banks x 50 slots.
#[test]
fn derives_slots_only_for_fixed_size_classes() {
    use nord_usb::wire::Status;

    let programs = Status {
        class: ObjectClass::Program,
        count: 380,
        free: 2820,
        used: 53580,
        dirty: 0,
        spare: 0,
    };
    assert_eq!(programs.bytes_per_item(), Some(141));
    assert_eq!(programs.slots(), Some(400));

    let set_lists = Status {
        class: ObjectClass::SetList,
        count: 63,
        free: 5206,
        used: 2394,
        dirty: 0,
        spare: 0,
    };
    assert_eq!(set_lists.bytes_per_item(), Some(38));
    assert_eq!(set_lists.slots(), Some(200));

    // Pianos genuinely vary in size, so there is no per-item constant to report.
    let pianos = Status {
        class: ObjectClass::Piano,
        count: 29,
        free: 1,
        used: 4012,
        dirty: 73,
        spare: 2,
    };
    assert_eq!(pianos.bytes_per_item(), None);
    assert_eq!(pianos.slots(), None);

    // An empty class must not divide by zero.
    let empty = Status {
        class: ObjectClass::Unknown(6),
        count: 0,
        free: 363,
        used: 0,
        dirty: 0,
        spare: 0,
    };
    assert_eq!(empty.slots(), None);
}

/// A library class's capacity is all four storage words, and only that sum holds still.
///
/// Both readings are off the same Electro 5 sample partition: the four words sum to
/// 2048 either way, while `free + used` reads 1983 in one and 1936 in the other. A
/// report built from those two makes the partition look like it is losing capacity
/// every time something is deleted, because a delete parks its space in `dirty`.
#[test]
fn a_library_capacity_is_all_four_words() {
    use nord_usb::wire::Status;

    let probed = Status {
        class: ObjectClass::Sample,
        count: 137,
        free: 47,
        used: 1936,
        dirty: 64,
        spare: 1,
    };
    // After a power cycle, with the same content: `free`'s prepared state does not
    // survive one, `dirty` does, and the sum is unchanged.
    let rebooted = Status {
        free: 0,
        dirty: 111,
        ..probed
    };

    assert_eq!(probed.total(), 2048);
    assert_eq!(rebooted.total(), 2048);

    // What a write can actually reach — the point of decoding `dirty` at all. The
    // rebooted partition reports zero free and is no less writable for it.
    assert_eq!(probed.available(), 111);
    assert_eq!(rebooted.available(), 111);
}

/// A per-item size is a property of the class, not of numbers that happen to divide.
///
/// Library content varies in size, so any exact division of its block counters is a
/// coincidence — and acting on one would report a slot count the class does not have.
#[test]
fn a_library_class_reports_no_per_item_size_however_its_counters_divide() {
    use nord_usb::wire::Status;

    let divisible = Status {
        class: ObjectClass::Sample,
        count: 4,
        free: 600,
        used: 400,
        dirty: 0,
        spare: 0,
    };
    assert_eq!(divisible.bytes_per_item(), None);
    assert_eq!(divisible.slots(), None);

    let slot_class = Status {
        class: ObjectClass::Program,
        ..divisible
    };
    assert_eq!(slot_class.bytes_per_item(), Some(100));
    assert_eq!(slot_class.slots(), Some(10));
}

/// The file a read rebuilds is a real `.ne5p`, not just the right bytes.
///
/// The replay compares reconstruction with the file saved for that slot; this checks it is
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
/// The framing is built rather than captured. The test checks three exchanges at offsets
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

    // Expected progress is independent of the production calculation.
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
