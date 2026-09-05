//! Synthetic replay tests for the set-list scan used before program moves.
//! Inferred from specimens; not confirmed on hardware.

#![cfg(feature = "replay")]

use nord_usb::op;
use nord_usb::transport::{Direction, ReplayTransport, Step};
use nord_usb::wire::{cmd, ui, Message, ObjectClass, Service};
use nord_usb::{Location, Session};

fn set_list(slot: u32) -> Location {
    Location { bank: 0, slot }
}

fn out(bytes: Vec<u8>) -> Step {
    Step {
        direction: Direction::Out,
        bytes,
    }
}

fn request(command: u32, args: &[u8]) -> Step {
    out(Message::new(Service::Program, 10, command, args.to_vec()).encode())
}

fn response(command: u32, rest: &[u8]) -> Step {
    Step {
        direction: Direction::In,
        bytes: Message::new(
            Service::Program,
            10,
            command,
            [&0u32.to_be_bytes()[..], rest].concat(),
        )
        .encode(),
    }
}

fn refusal(command: u32, status: u32) -> Step {
    Step {
        direction: Direction::In,
        bytes: Message::new(Service::Program, 10, command, status.to_be_bytes().to_vec()).encode(),
    }
}

fn slot_args(at: Location) -> Vec<u8> {
    let mut v = Vec::new();
    at.write_to(&mut v);
    v
}

/// A `0x1e` reply for a set list: the fields ahead of the name, the name, then the CRC.
fn info_reply(at: Location, version: u32, name: &str) -> Step {
    let mut args = slot_args(at);
    args.extend_from_slice(&18u32.to_be_bytes()); // body length
    args.extend_from_slice(b"ne5t");
    args.extend_from_slice(&version.to_be_bytes());
    args.extend_from_slice(&u32::MAX.to_be_bytes());
    args.extend_from_slice(&u32::MAX.to_be_bytes());
    args.extend_from_slice(&(name.len() as u32).to_be_bytes());
    args.extend_from_slice(name.as_bytes());
    args.extend_from_slice(&0u32.to_be_bytes());
    response(cmd::INFO + 1, &args)
}

/// A `0x28` reply for a set list: its four program slots, all live, addressed by
/// location with a null content id.
fn deps_reply(at: Location, programs: &[Location]) -> Step {
    let mut args = slot_args(at);
    args.extend_from_slice(&(programs.len() as u32).to_be_bytes());
    for p in programs {
        args.push(1); // live
        for w in [
            0u32,
            ObjectClass::Program.to_raw(),
            0, // id: slot-addressed content carries none
            0, // name length
            1, // has_location
            p.bank,
            p.slot,
        ] {
            args.extend_from_slice(&w.to_be_bytes());
        }
    }
    response(cmd::DEPENDENCIES + 1, &args)
}

fn session_open() -> Vec<Step> {
    vec![
        out(Message::new(Service::Ui, ui::SUBSYSTEM, ui::HELLO, Vec::new()).encode()),
        Step {
            direction: Direction::In,
            bytes: Message::new(Service::Ui, ui::SUBSYSTEM, ui::HELLO + 1, vec![0; 4]).encode(),
        },
        request(
            cmd::SESSION_OPEN,
            &ObjectClass::SetList.to_raw().to_be_bytes(),
        ),
        response(
            cmd::SESSION_OPEN + 1,
            &ObjectClass::SetList.to_raw().to_be_bytes(),
        ),
    ]
}

fn session_close() -> Vec<Step> {
    vec![
        request(cmd::SESSION_CLOSE, &[]),
        response(cmd::SESSION_CLOSE + 1, &[]),
        out(Message::new(Service::Ui, ui::SUBSYSTEM, ui::GOODBYE, Vec::new()).encode()),
        Step {
            direction: Direction::In,
            bytes: Message::new(Service::Ui, ui::SUBSYSTEM, ui::GOODBYE + 1, vec![0; 4]).encode(),
        },
    ]
}

/// The enumeration walk over one bank holding set lists at slots 0, 1 and 2.
///
/// It is the walk `op::occupied_slots` performs: an `INFO` probe at each bank's slot 0
/// to find out whether the bank exists at all, then the cursor from the bank's boundary
/// until it stops advancing, and a second bank answering out-of-range to end it.
fn walk_of_three() -> Vec<Step> {
    let mut steps = Vec::new();

    steps.push(request(cmd::INFO, &slot_args(set_list(0))));
    steps.push(info_reply(set_list(0), 1, "First"));

    let cursor = |from: u32| {
        let mut args = slot_args(Location {
            bank: 0,
            slot: from,
        });
        args.extend_from_slice(&0u32.to_be_bytes()); // direction: forward
        request(cmd::NEXT_SLOT, &args)
    };
    for (from, found) in [
        (op::SLOT_BOUNDARY, Some(0u32)),
        (0, Some(1)),
        (1, Some(2)),
        (2, None),
    ] {
        steps.push(cursor(from));
        match found {
            Some(slot) => steps.push(response(cmd::NEXT_SLOT + 1, &slot_args(set_list(slot)))),
            // Status 1 past the last occupied slot is how a walk ends.
            None => steps.push(refusal(cmd::NEXT_SLOT + 1, 1)),
        }
    }

    // The class has one bank, so the next probe is out of range.
    steps.push(request(
        cmd::INFO,
        &slot_args(Location { bank: 1, slot: 0 }),
    ));
    steps.push(refusal(cmd::INFO + 1, 3));
    steps
}

fn scan(steps: Vec<Step>, targets: &[Location]) -> (Vec<op::Referrer>, bool) {
    let mut t = ReplayTransport::new(steps);
    let found = pollster::block_on(async {
        let mut s = Session::open(&mut t, ObjectClass::SetList).await.unwrap();
        let r = op::set_lists_referencing(&mut s, targets).await;
        let closed = s.commit().await;
        closed.unwrap();
        r.unwrap()
    });
    let exhausted = t.is_exhausted();
    (found, exhausted)
}

#[test]
fn a_move_names_every_set_list_that_points_at_either_slot() {
    let moved = Location { bank: 0, slot: 6 }; // panel 1:7
    let destination = Location { bank: 6, slot: 9 }; // panel 7:10, occupied
    let untouched = Location { bank: 2, slot: 4 };

    let mut steps = session_open();
    steps.extend(walk_of_three());
    steps.push(request(cmd::DEPENDENCIES, &slot_args(set_list(0))));
    steps.push(deps_reply(set_list(0), &[moved, untouched]));
    steps.push(request(cmd::INFO, &slot_args(set_list(0))));
    steps.push(info_reply(set_list(0), 0, "Factory Set"));

    steps.push(request(cmd::DEPENDENCIES, &slot_args(set_list(1))));
    steps.push(deps_reply(set_list(1), &[untouched]));

    steps.push(request(cmd::DEPENDENCIES, &slot_args(set_list(2))));
    steps.push(deps_reply(set_list(2), &[destination]));
    steps.push(request(cmd::INFO, &slot_args(set_list(2))));
    steps.push(info_reply(set_list(2), 1, "Friday"));
    steps.extend(session_close());

    let (found, exhausted) = scan(steps, &[moved, destination]);
    assert!(exhausted, "did not consume the whole exchange");

    assert_eq!(found.len(), 2, "{found:#?}");
    assert_eq!(found[0].at, set_list(0));
    assert_eq!(found[0].name, "Factory Set");
    assert_eq!(found[0].programs, vec![moved]);
    assert_eq!(found[0].version, 0);

    assert_eq!(found[1].at, set_list(2));
    assert_eq!(found[1].name, "Friday");
    assert_eq!(found[1].programs, vec![destination]);
    assert_eq!(found[1].version, 1);
}

#[test]
fn a_repeated_reference_is_reported_once() {
    let moved = Location { bank: 0, slot: 6 };

    let mut steps = session_open();
    steps.extend(walk_of_three());
    steps.push(request(cmd::DEPENDENCIES, &slot_args(set_list(0))));
    steps.push(deps_reply(set_list(0), &[moved, moved, moved, moved]));
    steps.push(request(cmd::INFO, &slot_args(set_list(0))));
    steps.push(info_reply(set_list(0), 1, "All The Same"));
    for slot in [1, 2] {
        steps.push(request(cmd::DEPENDENCIES, &slot_args(set_list(slot))));
        steps.push(deps_reply(set_list(slot), &[]));
    }
    steps.extend(session_close());

    let (found, exhausted) = scan(steps, &[moved]);
    assert!(exhausted, "did not consume the whole exchange");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].programs, vec![moved]);
}

#[test]
fn a_program_nothing_references_reports_an_empty_list() {
    let moved = Location { bank: 7, slot: 49 };

    let mut steps = session_open();
    steps.extend(walk_of_three());
    for slot in [0, 1, 2] {
        steps.push(request(cmd::DEPENDENCIES, &slot_args(set_list(slot))));
        steps.push(deps_reply(
            set_list(slot),
            &[Location { bank: 0, slot: 1 }, Location { bank: 1, slot: 2 }],
        ));
    }
    steps.extend(session_close());

    let (found, exhausted) = scan(steps, &[moved]);
    assert!(exhausted, "did not consume the whole exchange");
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn rows_that_are_not_dependencies_are_not_referrers() {
    let moved = Location { bank: 0, slot: 6 };

    let mut deps = slot_args(set_list(0));
    deps.extend_from_slice(&3u32.to_be_bytes());
    deps.push(0);
    for w in [0u32, ObjectClass::Program.to_raw(), 0, 0, 1, 0, 6] {
        deps.extend_from_slice(&w.to_be_bytes());
    }
    deps.push(1);
    for w in [0u32, ObjectClass::Program.to_raw(), 0, 0, 0, 0, 0] {
        deps.extend_from_slice(&w.to_be_bytes());
    }
    deps.push(1);
    for w in [0u32, ObjectClass::Sample.to_raw(), 0, 0, 1, 0, 6] {
        deps.extend_from_slice(&w.to_be_bytes());
    }

    let mut steps = session_open();
    steps.extend(walk_of_three());
    steps.push(request(cmd::DEPENDENCIES, &slot_args(set_list(0))));
    steps.push(response(cmd::DEPENDENCIES + 1, &deps));
    for slot in [1, 2] {
        steps.push(request(cmd::DEPENDENCIES, &slot_args(set_list(slot))));
        steps.push(deps_reply(set_list(slot), &[]));
    }
    steps.extend(session_close());

    let (found, exhausted) = scan(steps, &[moved]);
    assert!(exhausted, "did not consume the whole exchange");
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn a_refused_walk_is_an_error_rather_than_an_empty_list() {
    let mut steps = session_open();
    steps.push(request(cmd::INFO, &slot_args(set_list(0))));
    steps.push(info_reply(set_list(0), 1, "First"));
    let mut args = slot_args(Location {
        bank: 0,
        slot: op::SLOT_BOUNDARY,
    });
    args.extend_from_slice(&0u32.to_be_bytes());
    steps.push(request(cmd::NEXT_SLOT, &args));
    steps.push(refusal(cmd::NEXT_SLOT + 1, op::ENUMERATION_DISABLED));

    let mut t = ReplayTransport::new(steps);
    let err = pollster::block_on(async {
        let mut s = Session::open(&mut t, ObjectClass::SetList).await.unwrap();
        let r = op::set_lists_referencing(&mut s, &[Location { bank: 0, slot: 6 }]).await;
        s.abort();
        r.unwrap_err()
    });
    assert!(
        matches!(
            err,
            nord_usb::Error::DeviceStatus(s) if s == op::ENUMERATION_DISABLED
        ),
        "{err}"
    );
}

/// The bound `op::set_lists_referencing` walks to before it gives up on a cursor that
/// will not terminate.
const WALK_CAP: u32 = 1024;

/// A cursor that keeps advancing forever must not have the slots it did report passed
/// off as the whole answer: "no set list is affected" is the one wrong thing to say
/// before a move.
#[test]
fn a_walk_that_never_ends_is_an_error_rather_than_a_truncated_list() {
    let mut steps = session_open();
    steps.push(request(cmd::INFO, &slot_args(set_list(0))));
    steps.push(info_reply(set_list(0), 1, "First"));

    let mut from = op::SLOT_BOUNDARY;
    for slot in 0..WALK_CAP {
        let mut args = slot_args(Location {
            bank: 0,
            slot: from,
        });
        args.extend_from_slice(&0u32.to_be_bytes()); // direction: forward
        steps.push(request(cmd::NEXT_SLOT, &args));
        steps.push(response(cmd::NEXT_SLOT + 1, &slot_args(set_list(slot))));
        from = slot;
    }

    let mut t = ReplayTransport::new(steps);
    let err = pollster::block_on(async {
        let mut s = Session::open(&mut t, ObjectClass::SetList).await.unwrap();
        let r = op::set_lists_referencing(&mut s, &[Location { bank: 0, slot: 6 }]).await;
        s.abort();
        r.unwrap_err()
    });

    assert!(t.is_exhausted(), "the walk stopped short of its bound");
    assert!(
        matches!(&err, nord_usb::Error::Transport(m) if m.contains("without ending")),
        "{err}"
    );
}

#[test]
fn an_empty_target_list_sends_nothing() {
    let mut t = ReplayTransport::new(session_open().into_iter().chain(session_close()).collect());
    let found = pollster::block_on(async {
        let mut s = Session::open(&mut t, ObjectClass::SetList).await.unwrap();
        let r = op::set_lists_referencing(&mut s, &[]).await;
        s.commit().await.unwrap();
        r.unwrap()
    });
    assert!(found.is_empty());
    assert!(t.is_exhausted());
}
