//! Session-driver behavior that operation replay intents cannot express.
//!
//! Committed scripts cover ordering and recovery. Declarative transports cover atypical
//! replies and read-limit routing. Neither needs hardware or platform support.

#![cfg(feature = "replay")]

#[path = "support/frames.rs"]
mod frames;
#[path = "support/scripts.rs"]
mod scripts;

use frames::{
    notify, refusal, reply, request, response, session_close, session_open, slot_args, ui_request,
    ui_response, words,
};
use nord_usb::transport::{ReplayTransport, Step, Transport};
use nord_usb::wire::{cmd, ui, Message, ObjectClass, Service};
use nord_usb::{op, Result, Session};
use std::collections::VecDeque;
use std::time::Duration;

fn replaying(name: &str) -> ReplayTransport {
    ReplayTransport::new(scripts::fixture(name).steps())
}

fn session_frames(class: ObjectClass, middle: Vec<Step>) -> ReplayTransport {
    ReplayTransport::new(
        session_open(class)
            .into_iter()
            .chain(middle)
            .chain(session_close())
            .collect(),
    )
}

/// A transport that answers from a list and records the read limit each answer was
/// asked for. An entry of `None` is the device saying nothing before the limit passed.
///
/// It accepts whatever is sent, so only a test about when the host reads belongs on it.
/// Everything else uses a script checked by the exact-match transport.
struct LimitTransport {
    replies: VecDeque<Option<Vec<u8>>>,
    limits: Vec<Duration>,
}

impl Transport for LimitTransport {
    async fn write(&mut self, _buf: &[u8]) -> Result<()> {
        Ok(())
    }

    async fn read(&mut self, _max: usize) -> Result<Vec<u8>> {
        unreachable!("the test transport only supports bounded reads")
    }

    async fn read_timeout(&mut self, _max: usize, limit: Duration) -> Result<Option<Vec<u8>>> {
        self.limits.push(limit);
        Ok(self.replies.pop_front().unwrap_or(None))
    }
}

#[test]
fn info_rejects_a_response_for_a_different_location() {
    let requested = nord_usb::Location { bank: 1, slot: 2 };
    let reported = words(&[
        0,
        3,
        0,
        u32::from_be_bytes(*b"ne5p"),
        4,
        u32::MAX,
        u32::MAX,
        0,
    ]);
    let mut t = session_frames(
        ObjectClass::Program,
        vec![
            request(cmd::INFO, &slot_args(requested)),
            response(cmd::INFO, &reported),
        ],
    );
    let err = pollster::block_on(async {
        let mut session = Session::open(&mut t, ObjectClass::Program).await.unwrap();
        let err = nord_usb::op::info(&mut session, requested)
            .await
            .expect_err("a mismatched INFO location must be rejected");
        session.commit().await.unwrap();
        err
    });
    assert!(matches!(
        err,
        nord_usb::Error::UnexpectedLocation {
            requested: got_requested,
            reported: nord_usb::Location { bank: 0, slot: 3 },
        } if got_requested == requested
    ));
    assert!(t.is_exhausted());
}

/// A frame whose checksum does not cover its bytes cannot be paired with the request
/// that asked for it, so the transaction is released.
#[test]
fn a_reply_whose_crc_is_wrong_is_refused_and_releases_the_session() {
    let at = nord_usb::Location { bank: 1, slot: 2 };
    let mut corrupt = response(cmd::INFO, &slot_args(at));
    *corrupt.bytes.last_mut().expect("the trailing CRC") ^= 0x01;

    let mut t = ReplayTransport::new(
        session_open(ObjectClass::Program)
            .into_iter()
            .chain([
                request(cmd::INFO, &slot_args(at)),
                corrupt,
                ui_request(ui::GOODBYE),
                ui_response(ui::GOODBYE, 0),
            ])
            .collect(),
    );
    let err = pollster::block_on(async {
        let mut session = Session::open(&mut t, ObjectClass::Program).await.unwrap();
        let err = op::info(&mut session, at)
            .await
            .expect_err("a frame whose CRC does not match its bytes is not a reply");
        session.commit().await.unwrap();
        err
    });

    assert!(matches!(err, nord_usb::Error::BadCrc { .. }), "{err}");
    assert!(
        t.is_exhausted(),
        "the desynchronized session did not send GOODBYE, leaving the device half-open"
    );
}

/// A request the device never answers desynchronizes every later reply, so the session
/// is released within the same transaction.
#[test]
fn a_request_that_reads_nothing_within_the_limit_releases_the_session() {
    let mut t = LimitTransport {
        replies: VecDeque::from([
            Some(ui_response(ui::HELLO, 0).bytes),
            Some(response(cmd::SESSION_OPEN, &[]).bytes),
            None,
            Some(ui_response(ui::GOODBYE, 0).bytes),
        ]),
        limits: Vec::new(),
    };
    let err = pollster::block_on(async {
        let mut session = Session::open(&mut t, ObjectClass::Program).await.unwrap();
        let err = op::status(&mut session)
            .await
            .expect_err("the device said nothing about the class within the read limit");
        session.commit().await.unwrap();
        err
    });

    assert!(matches!(err, nord_usb::Error::Transport(_)), "{err}");
    assert!(
        t.replies.is_empty(),
        "the GOODBYE reply was left unread, so the release never happened"
    );
}

#[test]
fn probe_surfaces_a_short_statusless_reply() {
    let command = 0x99;
    let mut t = session_frames(
        ObjectClass::Program,
        vec![
            request(command, &[1, 2]),
            reply(Message::new(
                Service::Program,
                frames::SUBSYSTEM,
                0x77,
                vec![0xab, 0xcd],
            )),
        ],
    );
    let (command, status, payload) = pollster::block_on(async {
        let mut session = Session::open(&mut t, ObjectClass::Program).await.unwrap();
        let reply = session
            .probe(
                Service::Program,
                10,
                command,
                &[1, 2],
                std::time::Duration::from_secs(1),
            )
            .await
            .unwrap()
            .unwrap();
        let observed = (reply.command, reply.status(), reply.payload().to_vec());
        session.commit().await.unwrap();
        observed
    });
    assert_eq!((command, status, payload), (0x77, None, vec![0xab, 0xcd]));
    assert!(t.is_exhausted());
}

#[test]
fn probe_limit_covers_close_without_changing_ordinary_reads() {
    let mut t = LimitTransport {
        replies: VecDeque::from([
            Some(ui_response(ui::HELLO, 0).bytes),
            Some(response(cmd::SESSION_OPEN, &[]).bytes),
            None,
            Some(response(cmd::STATUS, &words(&[1, 2, 3, 4, 5])).bytes),
            Some(response(cmd::SESSION_CLOSE, &[]).bytes),
            Some(ui_response(ui::GOODBYE, 0).bytes),
        ]),
        limits: Vec::new(),
    };
    pollster::block_on(async {
        let mut session = Session::open(&mut t, ObjectClass::Program).await.unwrap();
        assert!(session
            .probe(Service::Program, 10, 0x99, &[], Duration::from_secs(7),)
            .await
            .unwrap()
            .is_none());
        assert_eq!(op::status(&mut session).await.unwrap().count, 1);
        session
            .commit_with_read_limit(Duration::from_secs(7))
            .await
            .unwrap();
    });
    assert_eq!(
        t.limits,
        vec![
            nord_usb::session::READ_LIMIT,
            nord_usb::session::READ_LIMIT,
            Duration::from_secs(7),
            nord_usb::session::READ_LIMIT,
            Duration::from_secs(7),
            Duration::from_secs(7),
        ]
    );
}

#[test]
fn inventory_propagates_transport_failure_while_opening_a_class() {
    let mut transport = ReplayTransport::new(Vec::new());
    let err = pollster::block_on(op::inventory(&mut transport))
        .expect_err("a failed transport cannot describe an empty inventory");
    assert!(
        matches!(err, nord_usb::Error::Replay(_)),
        "wrong error: {err}"
    );
}

#[test]
fn inventory_propagates_a_malformed_status_after_closing_the_session() {
    // The sweep stops at the class that failed, so one class's frames are the whole
    // exchange.
    let class = ObjectClass::INVENTORY[0];
    let mut steps = session_open(class);
    steps.push(request(cmd::STATUS, &class.to_raw().to_be_bytes()));
    steps.push(response(cmd::STATUS, &words(&[1, 2])));
    steps.extend(session_close());

    let mut transport = ReplayTransport::new(steps);
    let err = pollster::block_on(op::inventory(&mut transport))
        .expect_err("a malformed status must not become an empty inventory");
    assert!(matches!(
        err,
        nord_usb::Error::Truncated { got: 8, need: 12 }
    ));
    assert!(
        transport.is_exhausted(),
        "the failed session was not closed"
    );
}

#[test]
fn inventory_skips_a_class_that_refuses_its_status() {
    let mut steps = Vec::new();
    for class in ObjectClass::INVENTORY {
        steps.extend(session_open(class));
        steps.push(request(cmd::STATUS, &class.to_raw().to_be_bytes()));
        steps.push(refusal(cmd::STATUS, 5));
        steps.extend(session_close());
    }

    let mut transport = ReplayTransport::new(steps);
    let statuses = pollster::block_on(op::inventory(&mut transport)).unwrap();
    assert!(statuses.is_empty());
    assert!(
        transport.is_exhausted(),
        "a refused status must still close its class session"
    );
}

/// The sample partition's block, so a one-byte body reserves exactly one block.
fn sample_unit() -> nord_usb::wire::AllocationUnit {
    let mut fields = 131_064u32.to_be_bytes().to_vec();
    fields.resize(29, 0);
    nord_usb::wire::Partition {
        index: ObjectClass::Sample.to_raw(),
        name: "Samp Lib".into(),
        native: false,
        fields,
    }
    .allocation_unit()
    .unwrap()
}

#[test]
fn inventory_skips_a_class_whose_session_the_device_refuses() {
    let mut steps = Vec::new();
    for class in ObjectClass::INVENTORY {
        steps.push(ui_request(ui::HELLO));
        steps.push(ui_response(ui::HELLO, 0));
        steps.push(request(cmd::SESSION_OPEN, &class.to_raw().to_be_bytes()));
        steps.push(refusal(cmd::SESSION_OPEN, 5));
        steps.push(ui_request(ui::GOODBYE));
        steps.push(ui_response(ui::GOODBYE, 0));
    }

    let mut transport = ReplayTransport::new(steps);
    let statuses = pollster::block_on(op::inventory(&mut transport)).unwrap();
    assert!(statuses.is_empty());
    assert!(
        transport.is_exhausted(),
        "a refused open must still release the UI session before the next class"
    );
}

#[test]
fn inventory_propagates_a_refused_hello_rather_than_reporting_nothing() {
    let mut transport = ReplayTransport::new(vec![
        ui_request(ui::HELLO),
        ui_response(ui::HELLO, 5),
        ui_request(ui::GOODBYE),
        ui_response(ui::GOODBYE, 0),
    ]);
    let err = pollster::block_on(op::inventory(&mut transport))
        .expect_err("a refused HELLO is not one class declining to answer");
    assert!(
        matches!(err, nord_usb::Error::DeviceStatus(5)),
        "wrong error: {err}"
    );
    assert!(
        transport.is_exhausted(),
        "the sweep carried on to another class after the UI refused it"
    );
}

#[test]
fn write_stops_on_a_malformed_cleaning_reply_without_polling_again() {
    let at = nord_usb::Location { bank: 0, slot: 0 };
    let file = nord_usb::envelope::wrap("ne5p", at, 4, &[1]).unwrap();
    let mut transport = session_frames(
        ObjectClass::Sample,
        vec![
            request(cmd::STATUS, &ObjectClass::Sample.to_raw().to_be_bytes()),
            response(cmd::STATUS, &words(&[0, 0, 0, 0, 0])),
            notify(ui::label("Cleaning...").unwrap()),
            notify(ui::percent(0)),
            request(cmd::WRITE_PREPARE, &1u32.to_be_bytes()),
            response(cmd::WRITE_PREPARE, &[]),
            request(cmd::WRITE_PREPARE_2, &[]),
            response(cmd::WRITE_PREPARE_2, &words(&[0, 0])),
        ],
    );
    let err = pollster::block_on(async {
        let session = Session::open(&mut transport, ObjectClass::Sample)
            .await
            .unwrap();
        let mut session = session.allow_destructive_writes();
        let err = op::write(&mut session, sample_unit(), at, &file, "sample", 0)
            .await
            .expect_err("a short cleaning reply is malformed");
        session.commit().await.unwrap();
        err
    });
    assert!(matches!(
        err,
        nord_usb::Error::Truncated { got: 8, need: 12 }
    ));
    assert!(
        transport.is_exhausted(),
        "a malformed cleaning reply triggered another poll"
    );
}

/// A refused class close still sends `GOODBYE` and returns the original error.
#[test]
fn a_failed_commit_reports_rather_than_panicking() {
    let mut t = replaying("session/refused_close.script");
    let err = pollster::block_on(async {
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

/// A queued `CHANGED` notification is surfaced and drained before the command reply.
#[test]
fn an_unsolicited_changed_notification_is_drained_not_mistaken_for_the_reply() {
    let mut t = replaying("session/changed_notification.script");
    pollster::block_on(async {
        let s = Session::open(&mut t, ObjectClass::Program).await.unwrap();
        assert!(
            s.instrument_changed(),
            "the drained notification must be surfaced, not silently skipped"
        );
        s.commit().await.unwrap();
    });
    assert!(t.is_exhausted(), "did not consume the whole exchange");
}

/// A notification flood stops at the drain cap and still releases the UI session.
#[test]
fn a_notification_flood_bails_rather_than_looping() {
    let mut t = replaying("session/notification_flood.script");
    let err = pollster::block_on(async {
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

/// A refused open clears a stale session and retries once.
#[test]
fn a_stale_session_is_cleared_and_the_open_retried() {
    let mut t = replaying("session/stale_session_retried.script");
    pollster::block_on(async {
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
