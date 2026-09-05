//! The facade: its session brackets, the geometry it reads once and keeps, and the two
//! things that geometry sizes — the enumeration walk and a library write's cleaning pass.
//!
//! The exchanges are the claim, so every trial asserts its script was consumed exactly.

#![cfg(feature = "replay")]

#[path = "support/scripts.rs"]
mod scripts;

use nord_usb::device::{Device, Product};
use nord_usb::transport::{Direction, ReplayTransport, Step};
use nord_usb::wire::{cmd, ui, Bank, Message, Partition, Service};
use nord_usb::{envelope, op, Error, Location, ObjectClass, Session};

fn out(bytes: Vec<u8>) -> Step {
    Step {
        direction: Direction::Out,
        bytes,
    }
}

fn r#in(bytes: Vec<u8>) -> Step {
    Step {
        direction: Direction::In,
        bytes,
    }
}

fn request(command: u32, args: &[u8]) -> Step {
    out(Message::new(Service::Program, 10, command, args.to_vec()).encode())
}

/// A successful reply to `command`: status `0`, then the device's own payload.
fn response(command: u32, payload: &[u8]) -> Step {
    r#in(
        Message::new(
            Service::Program,
            10,
            command + 1,
            [&0u32.to_be_bytes()[..], payload].concat(),
        )
        .encode(),
    )
}

fn refusal(command: u32, status: u32) -> Step {
    r#in(
        Message::new(
            Service::Program,
            10,
            command + 1,
            status.to_be_bytes().to_vec(),
        )
        .encode(),
    )
}

/// A fire-and-forget UI frame the device never answers.
fn notify(msg: Message) -> Step {
    out(msg.encode())
}

fn session_open(class: ObjectClass) -> Vec<Step> {
    vec![
        notify(Message::new(
            Service::Ui,
            ui::SUBSYSTEM,
            ui::HELLO,
            Vec::new(),
        )),
        r#in(Message::new(Service::Ui, ui::SUBSYSTEM, ui::HELLO + 1, vec![0; 4]).encode()),
        request(cmd::SESSION_OPEN, &class.to_raw().to_be_bytes()),
        response(cmd::SESSION_OPEN, &class.to_raw().to_be_bytes()),
    ]
}

fn session_close() -> Vec<Step> {
    vec![
        request(cmd::SESSION_CLOSE, &[]),
        response(cmd::SESSION_CLOSE, &[]),
        notify(Message::new(
            Service::Ui,
            ui::SUBSYSTEM,
            ui::GOODBYE,
            Vec::new(),
        )),
        r#in(Message::new(Service::Ui, ui::SUBSYSTEM, ui::GOODBYE + 1, vec![0; 4]).encode()),
    ]
}

fn words(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|w| w.to_be_bytes()).collect()
}

fn slot_args(at: Location) -> Vec<u8> {
    let mut v = Vec::new();
    at.write_to(&mut v);
    v
}

/// A chain that succeeded still owes the instrument its closing exchanges.
#[test]
fn a_read_bracket_closes_its_transaction() {
    let mut steps = session_open(ObjectClass::Program);
    steps.push(request(cmd::FOCUS, &[]));
    steps.push(response(cmd::FOCUS, &words(&[2, 13])));
    steps.extend(session_close());

    let mut device = Device::new(ReplayTransport::new(steps), Product::Unknown(0));
    let at = pollster::block_on(device.read(ObjectClass::Program, async |s| op::focus(s).await))
        .expect("the chain and its close both succeeded");

    assert_eq!(at, Location { bank: 2, slot: 13 });
    assert!(
        device.transport().is_exhausted(),
        "the bracket did not close"
    );
}

/// A device refusal leaves request and reply in step, so the bracket must still close —
/// and report what the chain hit rather than what the close returned.
#[test]
fn a_read_bracket_closes_after_a_failed_chain_and_reports_the_chains_error() {
    let at = Location { bank: 0, slot: 4 };
    let mut steps = session_open(ObjectClass::SetList);
    steps.push(request(cmd::INFO, &slot_args(at)));
    steps.push(refusal(cmd::INFO, 1));
    steps.extend(session_close());

    let mut device = Device::new(ReplayTransport::new(steps), Product::Unknown(0));
    // The script holds one `INFO`, so a chain that carried on past the refusal would
    // fail against the script rather than against the device.
    let err = pollster::block_on(device.read(ObjectClass::SetList, async |s| {
        op::info(s, at).await?;
        op::info(s, at).await
    }))
    .expect_err("an empty slot is status 1");

    assert!(matches!(err, Error::DeviceStatus(1)), "{err}");
    assert!(
        device.transport().is_exhausted(),
        "a failed chain left the transaction open"
    );
}

/// The tables the Electro 5 reports, from the committed recording of that exchange.
#[test]
fn geometry_is_read_once_and_reports_the_instruments_own_tables() {
    let steps = scripts::fixture("device/geometry.script").steps();
    let mut device = Device::new(ReplayTransport::new(steps), Product::Unknown(0));

    pollster::block_on(async {
        let geometry = device.geometry().await.expect("the recorded tables");

        assert_eq!(
            geometry.allocation_unit(ObjectClass::Piano).unwrap().get(),
            261_632
        );
        assert_eq!(
            geometry.allocation_unit(ObjectClass::Sample).unwrap().get(),
            131_064
        );
        assert!(geometry
            .allocation_unit(ObjectClass::Program)
            .unwrap()
            .is_bytes());

        for (class, banks, slots) in [
            (ObjectClass::Program, 8, 50),
            (ObjectClass::SetList, 4, 50),
            (ObjectClass::Piano, 6, 20),
            (ObjectClass::Sample, 1, 159),
            (ObjectClass::Live, 1, 3),
        ] {
            let declared = geometry.banks(class).unwrap();
            assert_eq!(declared.len(), banks, "{} banks", class.label());
            assert!(
                declared.iter().all(|b| b.slots == slots),
                "{} bank capacities: {declared:?}",
                class.label()
            );
        }
    });
    assert!(
        device.transport().is_exhausted(),
        "the geometry read did not consume the recording"
    );

    // Nothing is left to read, so a second call answering at all proves it sent nothing.
    pollster::block_on(async { device.geometry().await.expect("the cached tables") });
}

/// A class the partition table does not list has no geometry, and asking is an error
/// rather than a default that would size a destructive operation.
#[test]
fn a_class_the_instrument_does_not_have_is_an_error() {
    let steps = scripts::fixture("device/geometry.script").steps();
    let mut device = Device::new(ReplayTransport::new(steps), Product::Unknown(0));
    pollster::block_on(async {
        let geometry = device.geometry().await.unwrap();
        assert!(geometry.banks(ObjectClass::Unknown(9)).is_err());
        assert!(geometry.allocation_unit(ObjectClass::Unknown(9)).is_err());
    });
    assert!(device.transport().is_exhausted());
}

/// A refused `BANKS` is remembered as that partition's answer and re-reported, so a
/// walk of that class fails rather than running unbounded.
#[test]
fn a_refused_bank_list_is_reported_for_that_class_alone() {
    let mut steps = session_open(ObjectClass::Program);
    steps.push(request(cmd::PARTITIONS, &[]));
    steps.push(response(cmd::PARTITIONS, &partitions_payload(&[1, 100])));
    steps.push(request(cmd::BANKS, &0u32.to_be_bytes()));
    steps.push(response(
        cmd::BANKS,
        &banks_payload(0, &[("Bank 1", 50), ("Bank 2", 50)]),
    ));
    steps.push(request(cmd::BANKS, &1u32.to_be_bytes()));
    steps.push(refusal(cmd::BANKS, 0x15));
    steps.extend(session_close());

    let mut device = Device::new(ReplayTransport::new(steps), Product::Unknown(0));
    pollster::block_on(async {
        let geometry = device.geometry().await.unwrap();
        assert_eq!(geometry.banks(ObjectClass::Unknown(0)).unwrap().len(), 2);
        assert!(matches!(
            geometry.banks(ObjectClass::Piano),
            Err(Error::DeviceStatus(0x15))
        ));
        // The partition record still decodes; only its bank list was refused.
        assert_eq!(
            geometry.allocation_unit(ObjectClass::Piano).unwrap().get(),
            100
        );
    });
    assert!(device.transport().is_exhausted());
}

/// A `PARTITIONS` payload: a record count, then `[u32 name_len][name][29 field bytes]`
/// per partition, whose first field word is its allocation unit.
fn partitions_payload(units: &[u32]) -> Vec<u8> {
    let mut p = vec![units.len() as u8];
    for (index, unit) in units.iter().enumerate() {
        let name = format!("Partition {index}");
        p.extend_from_slice(&(name.len() as u32).to_be_bytes());
        p.extend_from_slice(name.as_bytes());
        let mut fields = unit.to_be_bytes().to_vec();
        fields.resize(29, 0);
        p.extend_from_slice(&fields);
    }
    p
}

/// A `BANKS` payload: the echoed partition, a count, then `[u32 name_len][name][u32
/// slots]` per bank.
fn banks_payload(partition: u32, banks: &[(&str, u32)]) -> Vec<u8> {
    let mut p = partition.to_be_bytes().to_vec();
    p.push(banks.len() as u8);
    for (name, slots) in banks {
        p.extend_from_slice(&(name.len() as u32).to_be_bytes());
        p.extend_from_slice(name.as_bytes());
        p.extend_from_slice(&slots.to_be_bytes());
    }
    p
}

fn partition_reporting(unit: u32) -> Partition {
    let mut fields = unit.to_be_bytes().to_vec();
    fields.resize(29, 0);
    Partition {
        index: 3,
        name: "Samp Lib".into(),
        native: false,
        fields,
    }
}

#[test]
fn a_body_is_rounded_up_to_whole_allocation_units() {
    let unit = partition_reporting(131_064).allocation_unit().unwrap();
    assert_eq!(unit.blocks_for(131_064).unwrap(), 1);
    assert_eq!(unit.blocks_for(131_065).unwrap(), 2);
    assert_eq!(unit.blocks_for(0).unwrap(), 0);
    assert!(!unit.is_bytes());
}

#[test]
fn a_block_count_past_the_wires_u32_is_an_error() {
    let unit = partition_reporting(1).allocation_unit().unwrap();
    assert!(unit.is_bytes());
    assert_eq!(unit.blocks_for(u32::MAX as usize).unwrap(), u32::MAX);
    assert!(unit.blocks_for(u32::MAX as usize + 1).is_err());
}

/// A unit of zero would size every write as needing nothing; a short record has no unit
/// at all. Neither may be read as "one byte".
#[test]
fn a_partition_that_states_no_usable_unit_is_an_error() {
    assert!(partition_reporting(0).allocation_unit().is_err());
    let short = Partition {
        fields: vec![0, 0, 1],
        ..partition_reporting(1)
    };
    assert!(matches!(
        short.allocation_unit(),
        Err(Error::Truncated { .. })
    ));
}

fn bank(index: u32, slots: u32) -> Bank {
    Bank {
        index,
        name: format!("Bank {}", index + 1),
        slots,
    }
}

/// One cursor request from `at`, and the answer the device gives it.
fn cursor(at: Location, answer: Option<Location>) -> Vec<Step> {
    let mut args = slot_args(at);
    // Direction, 0 = forward.
    args.extend_from_slice(&0u32.to_be_bytes());
    vec![
        request(cmd::NEXT_SLOT, &args),
        match answer {
            Some(found) => response(cmd::NEXT_SLOT, &slot_args(found)),
            // Status 1 past the last occupied slot is how the device ends a bank.
            None => refusal(cmd::NEXT_SLOT, 1),
        },
    ]
}

fn from_boundary(bank: u32) -> Location {
    Location {
        bank,
        slot: op::SLOT_BOUNDARY,
    }
}

/// Replay one walk. `steps` is everything after the session opens, closing exchanges
/// included where the walk is expected to reach them.
fn walk(banks: &[Bank], steps: Vec<Step>) -> (nord_usb::Result<Vec<Location>>, bool) {
    let mut t = ReplayTransport::new(
        session_open(ObjectClass::Program)
            .into_iter()
            .chain(steps)
            .collect(),
    );
    let found = pollster::block_on(async {
        let mut s = Session::open(&mut t, ObjectClass::Program).await.unwrap();
        let found = op::occupied_slots(&mut s, banks).await;
        match found.is_ok() {
            true => s.commit().await.unwrap(),
            // A walk that bailed owes nothing more; the frames it did not reach are the
            // report.
            false => s.abort(),
        }
        found
    });
    let exhausted = t.is_exhausted();
    (found, exhausted)
}

/// The walk visits every declared bank and leaves each one where the device says the
/// bank is over — an empty bank included.
#[test]
fn a_bounded_bank_ends_where_the_device_ends_it() {
    let banks = [bank(0, 3), bank(1, 3)];
    let hits = [Location { bank: 0, slot: 0 }, Location { bank: 0, slot: 2 }];
    let mut steps = cursor(from_boundary(0), Some(hits[0]));
    steps.extend(cursor(hits[0], Some(hits[1])));
    steps.extend(cursor(hits[1], None));
    steps.extend(cursor(from_boundary(1), None));
    steps.extend(session_close());

    let (found, exhausted) = walk(&banks, steps);
    assert!(exhausted, "the walk did not visit both declared banks");
    assert_eq!(found.unwrap(), hits);
}

/// A bank cannot hold more objects than the instrument says it has slots for. Reporting
/// the extras would mean trusting a cursor that has already contradicted the geometry.
#[test]
fn more_hits_than_the_bank_declares_is_an_error() {
    let banks = [bank(0, 2)];
    let hits = [
        Location { bank: 0, slot: 0 },
        Location { bank: 0, slot: 1 },
        Location { bank: 0, slot: 2 },
    ];
    let mut steps = cursor(from_boundary(0), Some(hits[0]));
    steps.extend(cursor(hits[0], Some(hits[1])));
    steps.extend(cursor(hits[1], Some(hits[2])));

    let (found, _) = walk(&banks, steps);
    assert!(
        matches!(
            found,
            Err(Error::Enumeration {
                bank: 0,
                answered,
                slots: 2
            }) if answered == hits[2]
        ),
        "{found:?}"
    );
}

/// A cursor answering the position it was asked about would spin forever.
#[test]
fn a_cursor_that_does_not_advance_is_an_error() {
    let banks = [bank(0, 50)];
    let stuck = Location { bank: 0, slot: 7 };
    let mut steps = cursor(from_boundary(0), Some(stuck));
    steps.extend(cursor(stuck, Some(stuck)));

    let (found, _) = walk(&banks, steps);
    assert!(
        matches!(found, Err(Error::Enumeration { bank: 0, answered, .. }) if answered == stuck),
        "{found:?}"
    );
}

/// The `(Native)` partitions report a sentinel instead of a capacity, so their banks
/// must not be held to any stated one.
#[test]
fn an_unbounded_bank_walks_past_a_bounded_banks_capacity() {
    let banks = [bank(0, Bank::UNBOUNDED)];
    assert!(!banks[0].is_bounded());

    let hits: Vec<Location> = (0..51).map(|slot| Location { bank: 0, slot }).collect();
    let mut steps = cursor(from_boundary(0), Some(hits[0]));
    for pair in hits.windows(2) {
        steps.extend(cursor(pair[0], Some(pair[1])));
    }
    steps.extend(cursor(*hits.last().unwrap(), None));
    steps.extend(session_close());

    let (found, exhausted) = walk(&banks, steps);
    assert!(exhausted, "the walk stopped inside the bank");
    assert_eq!(found.unwrap(), hits);
}

/// A library write reserves in the same transaction as the transfer, and reclaims the
/// shortfall between what the body needs and what the partition has free — the need
/// measured in the unit the instrument reported, not a constant.
///
/// The Electro 5 recordings under `sample/put-*` are the hardware evidence for the
/// frames; what is synthetic here is a body that spans more than one unit.
#[test]
fn a_library_write_reserves_the_shortfall_it_is_short_by() {
    let at = Location { bank: 0, slot: 98 };
    let body = vec![0xa5; 250];
    let file = envelope::wrap("nsmp", at, 1, &body).unwrap();
    let (name, timestamp) = ("two blocks", 1_787_522_949);

    // Unit 100: a 250-byte body is three blocks, and one is free, so two are reclaimed.
    let mut steps = session_open(ObjectClass::Program);
    steps.push(request(cmd::PARTITIONS, &[]));
    steps.push(response(
        cmd::PARTITIONS,
        &partitions_payload(&[1, 1, 1, 100]),
    ));
    for partition in 0..4u32 {
        steps.push(request(cmd::BANKS, &partition.to_be_bytes()));
        steps.push(response(
            cmd::BANKS,
            &banks_payload(partition, &[("Lib", 1)]),
        ));
    }
    steps.extend(session_close());

    steps.extend(session_open(ObjectClass::Sample));
    steps.push(request(
        cmd::STATUS,
        &ObjectClass::Sample.to_raw().to_be_bytes(),
    ));
    // count, free, used, dirty, spare.
    steps.push(response(cmd::STATUS, &words(&[138, 1, 2029, 18, 1])));
    steps.push(notify(ui::label("Cleaning...").unwrap()));
    steps.push(notify(ui::percent(0)));
    steps.push(request(cmd::WRITE_PREPARE, &2u32.to_be_bytes()));
    steps.push(response(cmd::WRITE_PREPARE, &[]));
    steps.push(request(cmd::WRITE_PREPARE_2, &[]));
    // requested, done, running: back to 0 is the pass reporting itself finished.
    steps.push(response(cmd::WRITE_PREPARE_2, &words(&[2, 2, 0])));
    steps.push(notify(ui::percent(100)));

    steps.push(notify(ui::label("Downloading...").unwrap()));
    let mut begin = slot_args(at);
    begin.extend_from_slice(&words(&[body.len() as u32]));
    begin.extend_from_slice(b"nsmp");
    begin.extend_from_slice(&words(&[timestamp, u32::MAX, name.len() as u32]));
    begin.extend_from_slice(name.as_bytes());
    steps.push(request(cmd::BEGIN_WRITE, &begin));
    steps.push(response(cmd::BEGIN_WRITE, &slot_args(at)));
    let mut data = slot_args(at);
    data.extend_from_slice(&words(&[0, body.len() as u32]));
    data.extend_from_slice(&body);
    steps.push(request(cmd::WRITE_DATA, &data));
    steps.push(response(cmd::WRITE_DATA, &slot_args(at)));
    steps.push(notify(ui::percent(100)));
    steps.push(request(cmd::END_TRANSFER, &slot_args(at)));
    steps.push(response(cmd::END_TRANSFER, &slot_args(at)));
    steps.extend(session_close());

    let mut device = Device::new(ReplayTransport::new(steps), Product::Unknown(0));
    pollster::block_on(device.write(ObjectClass::Sample, at, &file, name, timestamp))
        .expect("the reserve and the transfer share one transaction");
    assert!(
        device.transport().is_exhausted(),
        "the write did not send the frames the script holds"
    );
}

/// A slot class has no blocks to reserve, so nothing precedes its transfer.
#[test]
fn a_slot_class_write_sends_no_reserve_step() {
    let at = Location { bank: 6, slot: 9 };
    let body = vec![0x11; 40];
    let file = envelope::wrap("ne5t", at, 1, &body).unwrap();
    let (name, timestamp) = ("Friday", 1_787_428_287);

    let mut steps = session_open(ObjectClass::SetList);
    steps.push(notify(ui::label("Downloading...").unwrap()));
    let mut begin = slot_args(at);
    begin.extend_from_slice(&words(&[body.len() as u32]));
    begin.extend_from_slice(b"ne5t");
    begin.extend_from_slice(&words(&[timestamp, u32::MAX, name.len() as u32]));
    begin.extend_from_slice(name.as_bytes());
    steps.push(request(cmd::BEGIN_WRITE, &begin));
    steps.push(response(cmd::BEGIN_WRITE, &slot_args(at)));
    let mut data = slot_args(at);
    data.extend_from_slice(&words(&[0, body.len() as u32]));
    data.extend_from_slice(&body);
    steps.push(request(cmd::WRITE_DATA, &data));
    steps.push(response(cmd::WRITE_DATA, &slot_args(at)));
    steps.push(notify(ui::percent(100)));
    steps.push(request(cmd::END_TRANSFER, &slot_args(at)));
    steps.push(response(cmd::END_TRANSFER, &slot_args(at)));
    steps.extend(session_close());

    let mut device = Device::new(ReplayTransport::new(steps), Product::Unknown(0));
    pollster::block_on(device.write(ObjectClass::SetList, at, &file, name, timestamp))
        .expect("a set list write is the transfer alone");
    assert!(device.transport().is_exhausted());
}

#[test]
fn a_product_id_names_the_instrument_it_belongs_to() {
    assert_eq!(
        Product::from_product_id(nord_usb::transport::PRODUCT_ID_ELECTRO5),
        Product::Electro5
    );
    assert_eq!(Product::from_product_id(0x1234), Product::Unknown(0x1234));
}
