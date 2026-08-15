//! Replay tests for the [`Device`] layer: brackets and intents.
//!
//! Unlike `ops.rs`, which pins each operation's bytes against NSM captures, these
//! scripts pin *composition*: which operations an intent runs, in which order, inside
//! how many transactions, and that the closing exchanges run on every path. Request
//! frames are therefore built with this crate's own encoder — byte fidelity is the op
//! tests' job — except the session wrapper, which stays the captured hex.

#![cfg(feature = "replay")]

use nord_usb::device::{Device, Occupant, Product};
use nord_usb::transport::{Direction, ReplayTransport, Step};
use nord_usb::wire::{cmd, ui, ObjectClass};
use nord_usb::{Location, Message, Service};

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

/// The captured session open/close wrapper (see `ops.rs`), with `middle` spliced in.
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

/// A host request on the program service.
fn req(command: u32, args: Vec<u8>) -> Step {
    Step {
        direction: Direction::Out,
        bytes: Message::new(Service::Program, 10, command, args).encode(),
    }
}

/// A device response: `status` word, then `payload`.
fn resp(command: u32, status: u32, payload: &[u8]) -> Step {
    let mut args = status.to_be_bytes().to_vec();
    args.extend_from_slice(payload);
    Step {
        direction: Direction::In,
        bytes: Message::new(Service::Program, 10, command, args).encode(),
    }
}

/// A fire-and-forget UI progress frame, exactly as the ops emit it.
fn ui_label(text: &str) -> Step {
    Step {
        direction: Direction::Out,
        bytes: ui::label(text).unwrap().encode(),
    }
}

fn ui_percent(pct: u16) -> Step {
    Step {
        direction: Direction::Out,
        bytes: ui::percent(pct).encode(),
    }
}

fn loc_bytes(at: Location) -> Vec<u8> {
    let mut v = Vec::new();
    at.write_to(&mut v);
    v
}

/// The `FOCUS` exchange: no arguments, reply carries the focused bank/slot.
fn focus_exchange(focus: Location) -> Vec<Step> {
    vec![
        req(cmd::FOCUS, Vec::new()),
        resp(cmd::FOCUS + 1, 0, &loc_bytes(focus)),
    ]
}

/// An `INFO` reply payload in the shape `ProgramInfo::decode` expects: bank, slot,
/// body_len, format, version, two opaque words, name, and a trailing `0xffffffff`
/// standing for "no checksum" so the read path has nothing to verify against.
fn info_payload(at: Location, body_len: u32, name: &str) -> Vec<u8> {
    let mut p = loc_bytes(at);
    p.extend_from_slice(&body_len.to_be_bytes());
    p.extend_from_slice(b"ne5p");
    p.extend_from_slice(&4u32.to_be_bytes());
    p.extend_from_slice(&u32::MAX.to_be_bytes());
    p.extend_from_slice(&u32::MAX.to_be_bytes());
    p.extend_from_slice(&(name.len() as u32).to_be_bytes());
    p.extend_from_slice(name.as_bytes());
    p.extend_from_slice(&u32::MAX.to_be_bytes());
    p
}

/// The frames `op::write_program` puts on the wire for a body small enough to go out
/// in one `WRITE_DATA`, up to and including the exchange at which it is refused —
/// `refused_at_write_data` truncates the sequence after the device answers status 4.
fn write_exchange(
    at: Location,
    body: &[u8],
    timestamp: u32,
    refused_at_write_data: bool,
) -> Vec<Step> {
    let mut begin = loc_bytes(at);
    begin.extend_from_slice(&(body.len() as u32).to_be_bytes());
    begin.extend_from_slice(b"ne5p");
    begin.extend_from_slice(&timestamp.to_be_bytes());
    begin.extend_from_slice(&u32::MAX.to_be_bytes());
    begin.extend_from_slice(&1u32.to_be_bytes());
    begin.push(b'0');

    let mut data = loc_bytes(at);
    data.extend_from_slice(&0u32.to_be_bytes());
    data.extend_from_slice(&(body.len() as u32).to_be_bytes());
    data.extend_from_slice(body);

    let mut v = vec![
        ui_label("Downloading..."),
        req(cmd::BEGIN_WRITE, begin),
        resp(cmd::BEGIN_WRITE + 1, 0, &[]),
        req(cmd::WRITE_DATA, data),
    ];
    if refused_at_write_data {
        v.push(resp(cmd::WRITE_DATA + 1, 4, &[]));
        return v;
    }
    v.extend([
        resp(cmd::WRITE_DATA + 1, 0, &[]),
        ui_percent(100),
        req(cmd::END_TRANSFER, loc_bytes(at)),
        resp(cmd::END_TRANSFER + 1, 0, &[]),
    ]);
    v
}

/// The frames `op::read_program` puts on the wire for a single-chunk body: `INFO`,
/// the progress label, `BEGIN_READ`, one `READ`, the bar, `END_TRANSFER`.
fn read_exchange(at: Location, body: &[u8], name: &str) -> Vec<Step> {
    let mut read_req = loc_bytes(at);
    read_req.extend_from_slice(&0u32.to_be_bytes());
    read_req.extend_from_slice(&(body.len() as u32).to_be_bytes());

    let mut read_resp = loc_bytes(at);
    read_resp.extend_from_slice(&0u32.to_be_bytes());
    read_resp.extend_from_slice(&(body.len() as u32).to_be_bytes());
    read_resp.extend_from_slice(body);

    vec![
        req(cmd::INFO, loc_bytes(at)),
        resp(cmd::INFO + 1, 0, &info_payload(at, body.len() as u32, name)),
        ui_label("Uploading..."),
        req(cmd::BEGIN_READ, loc_bytes(at)),
        resp(cmd::BEGIN_READ + 1, 0, &[]),
        req(cmd::READ, read_req),
        resp(cmd::READ + 1, 0, &read_resp),
        ui_percent(100),
        req(cmd::END_TRANSFER, loc_bytes(at)),
        resp(cmd::END_TRANSFER + 1, 0, &[]),
    ]
}

/// Minimal executor, as in `ops.rs`: replayed exchanges never pend.
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

fn device(script: Vec<Step>) -> Device<ReplayTransport> {
    Device::new(ReplayTransport::new(script), Product::Electro5.profile())
}

/// Two operations chained in a bracket share one transaction: one open, one close.
#[test]
fn a_bracket_chains_operations_in_one_transaction() {
    let focus = Location::from_user(2, 12);
    let mut middle = focus_exchange(focus);
    // The captured SELECT of 2:12 (`open_on_device_2-12`).
    middle.push(step(
        Direction::Out,
        "0000001a0000000c0000000a0000002f000000010000000b746a",
    ));
    middle.push(step(
        Direction::In,
        "0000001e0000000c0000000a0000003000000000000000010000000b19df",
    ));
    let mut d = device(wrap(middle));

    let selected = block_on(d.read(ObjectClass::Program, async |t| {
        let at = t.focus().await?;
        t.select(at).await?;
        Ok(at)
    }))
    .unwrap();

    assert_eq!(selected, focus);
    assert!(
        d.transport().is_exhausted(),
        "the chain did not run inside exactly one transaction"
    );
}

/// A chain that bails mid-way still gets its closing exchanges, and the caller gets
/// the operation's error — the bracket is the always-commit discipline made a path.
#[test]
fn a_mid_chain_refusal_still_commits() {
    let at = Location::from_user(9, 1);
    let middle = vec![
        req(cmd::INFO, loc_bytes(at)),
        // Status 3: out of range. A refusal, not a desync — the session stays usable
        // and the close must still run.
        resp(cmd::INFO + 1, 3, &[]),
    ];
    let mut d = device(wrap(middle));

    let err = block_on(d.read(ObjectClass::Program, async |t| t.info(at).await.map(|_| ())))
        .expect_err("the refusal was swallowed");

    assert!(
        matches!(err, nord_usb::Error::DeviceStatus(3)),
        "wrong error: {err}"
    );
    assert!(
        d.transport().is_exhausted(),
        "the failed chain did not run the closing exchanges"
    );
}

/// Replacing an empty slot the panel has focused: no delete, and the write ends with
/// the re-select that makes the panel pick up the new content — all one transaction.
#[test]
fn replace_of_an_empty_focused_slot_writes_and_refocuses() {
    let at = Location::from_user(6, 12);
    let timestamp = 0x1122_3344;
    let body = b"NEWBYTES";
    let file = nord_usb::envelope::wrap("ne5p", at, 4, body).unwrap();

    let mut middle = focus_exchange(at);
    middle.push(req(cmd::INFO, loc_bytes(at)));
    middle.push(resp(cmd::INFO + 1, 1, &[])); // vacant
    middle.extend(write_exchange(at, body, timestamp, false));
    // The refocus: SELECT of the slot just written.
    middle.push(req(cmd::SELECT, loc_bytes(at)));
    middle.push(resp(cmd::SELECT + 1, 0, &loc_bytes(at)));
    let mut d = device(wrap(middle));

    let outcome = block_on(d.replace_program(ObjectClass::Program, at, &file, timestamp)).unwrap();

    assert_eq!(outcome.previous, None);
    assert!(outcome.refocused, "the focused slot was not re-selected");
    assert!(d.transport().is_exhausted(), "unexpected transaction shape");
}

/// A slot the panel is *not* focused on gets no re-select.
#[test]
fn replace_away_from_the_focus_does_not_select() {
    let at = Location::from_user(6, 12);
    let timestamp = 0x1122_3344;
    let body = b"NEWBYTES";
    let file = nord_usb::envelope::wrap("ne5p", at, 4, body).unwrap();

    let mut middle = focus_exchange(Location::from_user(1, 1));
    middle.push(req(cmd::INFO, loc_bytes(at)));
    middle.push(resp(cmd::INFO + 1, 1, &[]));
    middle.extend(write_exchange(at, body, timestamp, false));
    let mut d = device(wrap(middle));

    let outcome = block_on(d.replace_program(ObjectClass::Program, at, &file, timestamp)).unwrap();

    assert!(!outcome.refocused);
    assert!(d.transport().is_exhausted(), "unexpected transaction shape");
}

/// The whole replace dance against an occupied slot whose write fails: the occupant is
/// read back before the delete, the refused write closes its transaction properly, and
/// a *separate* recovery transaction puts the backup bytes back.
#[test]
fn a_failed_replace_restores_the_occupant_in_a_recovery_transaction() {
    let at = Location::from_user(6, 12);
    let timestamp = 0x1122_3344;
    let old_body = b"OLDBYTES";
    let new_body = b"NEWBYTES";
    let file = nord_usb::envelope::wrap("ne5p", at, 4, new_body).unwrap();

    // Transaction 1: focus elsewhere, occupant found, backed up, deleted — then the
    // write is refused at WRITE_DATA with status 4, and the close still runs.
    let mut middle = focus_exchange(Location::from_user(1, 1));
    middle.push(req(cmd::INFO, loc_bytes(at)));
    middle.push(resp(
        cmd::INFO + 1,
        0,
        &info_payload(at, old_body.len() as u32, "Old"),
    ));
    middle.extend(read_exchange(at, old_body, "Old"));
    middle.push(ui_label("Deleting..."));
    middle.push(req(cmd::DELETE, loc_bytes(at)));
    middle.push(resp(cmd::DELETE + 1, 0, &loc_bytes(at)));
    middle.extend(write_exchange(at, new_body, timestamp, true));
    let mut script = wrap(middle);

    // Transaction 2, the recovery: the backup — which `read_program` wrapped into a
    // file — written back into the now-empty slot.
    script.extend(wrap(write_exchange(at, old_body, timestamp, false)));

    let mut d = device(script);
    let err = block_on(d.replace_program(ObjectClass::Program, at, &file, timestamp))
        .expect_err("the refused write was reported as success");

    assert!(
        matches!(err.error, nord_usb::Error::DeviceStatus(4)),
        "wrong error: {}",
        err.error
    );
    assert!(
        matches!(err.occupant, Occupant::Restored),
        "the occupant was not restored: {:?}",
        err.occupant
    );
    assert!(
        d.transport().is_exhausted(),
        "the recovery transaction did not run, or ran wrong"
    );
}

/// `update_focused` resolves the focus in a read-only transaction, then replaces at
/// it — which, being the focused slot, ends with the re-select.
#[test]
fn update_focused_lands_on_the_panels_slot() {
    let at = Location::from_user(3, 7);
    let timestamp = 0x1122_3344;
    let body = b"NEWBYTES";
    let file = nord_usb::envelope::wrap("ne5p", at, 4, body).unwrap();

    // Transaction 1: just the focus read.
    let mut script = wrap(focus_exchange(at));
    // Transaction 2: the replace, seeing the same focus.
    let mut middle = focus_exchange(at);
    middle.push(req(cmd::INFO, loc_bytes(at)));
    middle.push(resp(cmd::INFO + 1, 1, &[]));
    middle.extend(write_exchange(at, body, timestamp, false));
    middle.push(req(cmd::SELECT, loc_bytes(at)));
    middle.push(resp(cmd::SELECT + 1, 0, &loc_bytes(at)));
    script.extend(wrap(middle));

    let mut d = device(script);
    let (landed, outcome) =
        block_on(d.update_focused(ObjectClass::Program, &file, timestamp)).unwrap();

    assert_eq!(landed, at);
    assert!(outcome.refocused);
    assert!(d.transport().is_exhausted(), "unexpected transaction shape");
}

#[test]
fn an_unknown_product_gets_the_conservative_profile() {
    let p = Product::from_product_id(0x9999).profile();
    assert_eq!(p.product, Product::Unknown(0x9999));
    assert!(!p.overwrite_in_place);
    assert_eq!(p.inventory, &[ObjectClass::Program]);

    let e5 = Product::from_product_id(0x0027).profile();
    assert_eq!(e5.product, Product::Electro5);
    assert_eq!(e5.inventory.len(), 4);
}
