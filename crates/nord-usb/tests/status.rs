//! What the read path decodes, and how strictly a replay checks what is sent.
//!
//! The exchanges live in `tests/scripts`, and `tests/replay` checks their bytes. These
//! tests assert what the bytes mean: the counters a `STATUS` reply carries, the
//! container a read rebuilds, and the chunking of a body larger than one request. They
//! also cover two properties of the replay transport that only a wrong caller can show.
//!
//! They need no hardware and run anywhere the crate compiles, including under Wine,
//! qemu, and wasm.

#![cfg(feature = "replay")]

#[path = "support/frames.rs"]
mod frames;
#[path = "support/scripts.rs"]
mod scripts;

use frames::{notify, request, response, session_close, session_open, slot_args};
use nord_usb::op;
use nord_usb::transport::ReplayTransport;
use nord_usb::wire::ObjectClass;
use nord_usb::Session;

/// The program-class transaction Nord Sound Manager produced, as the sweep drives it.
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
    // A slot class holds no dirty space: deleting a program returns its bytes to
    // `free`, so both trailing words stay zero here.
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

/// A reply that stops after the three counters every class reports still decodes, with
/// the missing words as zero.
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
/// refused.
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
fn a_status_reply_truncated_inside_an_optional_word_is_refused() {
    use nord_usb::wire::Status;

    let full = counters(&[375, 3525, 52875, 64, 1]);
    for (len, need) in [(13, 16), (15, 16), (17, 20), (19, 20)] {
        let err = Status::decode(ObjectClass::Sample, &status_reply(&full[..len]))
            .expect_err("a partial counter is not a shorter STATUS shape");
        assert!(
            matches!(err, nord_usb::Error::Truncated { got, need: n } if got == len && n == need),
            "{err}"
        );
    }
}

#[test]
fn an_exact_replay_rejects_a_frame_that_differs_from_the_script() {
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
        matches!(err, Some(nord_usb::Error::Replay(_))),
        "opening the wrong object class was not rejected: {err:?}"
    );
    assert_eq!(
        t.position(),
        2,
        "the HELLO exchange is in the script and the class it opens is not, so only the \
         first two frames may be consumed"
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
/// Numbers are from an Electro 5: adding one program moved `used` by 141
/// (53439 -> 53580), which is 121 body + 16 name + 4 CRC, and 56400 / 141 is 400, the
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

    // Pianos vary in size, so there is no per-item constant.
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

/// A library class's capacity is the sum of all four storage words, which stays
/// constant.
///
/// Both readings are from the same Electro 5 sample partition: the four words sum to
/// 2048 either way, while `free + used` reads 1983 in one and 1936 in the other. A
/// report built from those two would show the partition losing capacity with every
/// delete, because a delete moves its space into `dirty`.
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

    // What a write can reach. The rebooted partition reports zero free and is just as
    // writable.
    assert_eq!(probed.available(), 111);
    assert_eq!(rebooted.available(), 111);
}

/// Library content varies in size, so an exact division of its block counters is a
/// coincidence, and acting on it would report a slot count the class does not have.
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

/// The replay compares a rebuilt file with the one saved for that slot. This checks
/// that the saved file is a `.ne5p` container whose header carries the format tag and
/// the address, which the wire never sends together.
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
/// advance by what was asked for.
///
/// The framing is built, not captured: three exchanges at offsets 0 / 32720 / 65440
/// with lengths 32720 / 32720 / 777, in that order, under an exact-match transport. A
/// single whole-body request, a wrong offset, or a dropped final chunk fails it.
#[test]
fn a_large_body_is_read_in_chunks() {
    use nord_usb::wire::{cmd, ui};

    const CHUNK: u32 = 32720;
    const TAIL: u32 = 777;
    let body_len = CHUNK * 2 + TAIL;

    // Position-dependent, so chunks reassembled out of order or with a gap are caught.
    let body: Vec<u8> = (0..body_len).map(|i| (i % 251) as u8).collect();

    // bank 8 slot 14 -> 7, 13 on the wire.
    let at = nord_usb::Location::from_user(8, 14);
    let slot = slot_args(at);

    let mut info_args = slot.clone();
    info_args.extend_from_slice(&body_len.to_be_bytes());
    info_args.extend_from_slice(b"ne5p");
    info_args.extend_from_slice(&4u32.to_be_bytes()); // version
    info_args.extend_from_slice(&u32::MAX.to_be_bytes());
    info_args.extend_from_slice(&u32::MAX.to_be_bytes());
    info_args.extend_from_slice(&8u32.to_be_bytes()); // name length
    info_args.extend_from_slice(b"chunked ");
    info_args.extend_from_slice(&0u32.to_be_bytes()); // crc32: none

    let mut script = session_open(ObjectClass::Program);
    script.extend([
        request(cmd::INFO, &slot),
        response(cmd::INFO, &info_args),
        notify(ui::label("Uploading...").unwrap()),
        request(cmd::BEGIN_READ, &slot),
        response(cmd::BEGIN_READ, &slot),
    ]);

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
        script.push(response(cmd::READ, &resp));
        script.push(notify(ui::percent(pct)));
    }

    script.push(request(cmd::END_TRANSFER, &slot));
    script.push(response(cmd::END_TRANSFER, &slot));
    script.extend(session_close());

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

/// The allocation unit of a byte-granular class, so a write reserves nothing and sends
/// only the transfer.
fn byte_granular_unit(class: ObjectClass) -> nord_usb::wire::AllocationUnit {
    let mut fields = 1u32.to_be_bytes().to_vec();
    fields.resize(29, 0);
    nord_usb::wire::Partition {
        index: class.to_raw(),
        name: "Prog".into(),
        native: false,
        fields,
    }
    .allocation_unit()
    .unwrap()
}

/// A body larger than one `WRITE_DATA` leaves in several frames, and only the last is
/// acknowledged. The device does not answer the others, so a reply scripted for one
/// would be read as the answer to a later request.
///
/// The framing is built, not captured: three chunks at offsets 0 / 32720 / 65440 with
/// lengths 32720 / 32720 / 777, in that order, under an exact-match transport. A
/// whole-body single frame, a wrong offset, an acknowledged intermediate chunk, or a
/// dropped tail fails it.
#[test]
fn a_large_body_is_written_in_chunks() {
    use nord_usb::wire::{cmd, ui, Message, Service};

    const CHUNK: usize = 32720;
    const TAIL: usize = 777;

    let at = nord_usb::Location::from_user(8, 14);
    // Position-dependent, so a chunk sent from the wrong offset is caught.
    let body: Vec<u8> = (0..CHUNK * 2 + TAIL).map(|i| (i % 251) as u8).collect();
    let file = nord_usb::envelope::wrap("ne5p", at, 4, &body).unwrap();
    let (name, timestamp) = ("chunked", 1_787_428_287);

    let mut script = session_open(ObjectClass::Program);
    script.push(notify(ui::label("Downloading...").unwrap()));
    script.push(request(
        cmd::BEGIN_WRITE,
        &op::begin_write_args(at, body.len(), b"ne5p", timestamp, name).unwrap(),
    ));
    script.push(response(cmd::BEGIN_WRITE, &slot_args(at)));

    // Expected progress is independent of the production calculation.
    for (offset, len, pct, acknowledged) in [
        (0, CHUNK, 49u16, false),
        (CHUNK, CHUNK, 98, false),
        (CHUNK * 2, TAIL, 100, true),
    ] {
        let args = op::write_data_args(at, offset, &body[offset..offset + len]).unwrap();
        match acknowledged {
            true => {
                script.push(request(cmd::WRITE_DATA, &args));
                script.push(response(cmd::WRITE_DATA, &slot_args(at)));
            }
            false => script.push(notify(Message::new(
                Service::Program,
                frames::SUBSYSTEM,
                cmd::WRITE_DATA,
                args,
            ))),
        }
        script.push(notify(ui::percent(pct)));
    }

    script.push(request(cmd::END_TRANSFER, &slot_args(at)));
    script.push(response(cmd::END_TRANSFER, &slot_args(at)));
    script.extend(session_close());

    let mut t = ReplayTransport::new(script);
    pollster::block_on(async {
        let mut s = Session::open(&mut t, ObjectClass::Program)
            .await
            .unwrap()
            .allow_destructive_writes();
        let written = op::write(
            &mut s,
            byte_granular_unit(ObjectClass::Program),
            at,
            &file,
            name,
            timestamp,
        )
        .await;
        let closed = s.commit().await;
        written.expect("the chunked write");
        closed.expect("the transaction closed");
    });
    assert!(t.is_exhausted(), "did not consume the whole exchange");
}
