//! The frames a synthetic exchange is built from: one builder per shape, so a script
//! written in one test target means the same thing in another.
//!
//! A response is named by the command it answers; the builder adds the `+ 1` and the
//! status word ahead of the payload.
//!
//! ⚠️ A support module, not a test target: each test target that includes it
//! compiles its own copy.
#![allow(dead_code)]

use nord_usb::transport::Step;
use nord_usb::wire::{cmd, ui, Classes, Location, Message, ObjectClass, Partition, Service};

/// The Electro 5's partition names in table order, as `device/geometry.script` records
/// them.
pub const ELECTRO5: [&str; 8] = [
    "Piano (Native)",
    "Piano",
    "Samp Lib (Native)",
    "Samp Lib",
    "Program",
    "Set List",
    "Live",
    "Settings",
];

/// Where the Electro 5's table puts each class, as `device/geometry.script` records it.
pub fn e5_index(class: ObjectClass) -> u32 {
    match class {
        ObjectClass::Piano => 1,
        ObjectClass::Sample => 3,
        ObjectClass::Program => 4,
        ObjectClass::SetList => 5,
        ObjectClass::Live => 6,
        ObjectClass::Settings => 7,
        ObjectClass::OrganPreset | ObjectClass::PianoPreset | ObjectClass::SynthPreset => {
            unreachable!("the Electro 5 keeps no presets apart from its programs")
        }
        ObjectClass::Unknown(index) => index,
    }
}

/// Partitions with these names in table order, each with an empty record.
pub fn partitions(names: &[&str]) -> Vec<Partition> {
    (0..)
        .zip(names)
        .map(|(index, name)| Partition {
            index,
            name: name.to_string(),
            native: name.contains("(Native)"),
            fields: vec![0; 29],
        })
        .collect()
}

/// The Electro 5's table, for opening a session as an Electro 5 is addressed.
pub fn electro5() -> Classes {
    Classes::of(&partitions(&ELECTRO5))
}

/// The subsystem every [`Service::Program`] frame carries.
pub const SUBSYSTEM: u32 = 10;

/// A frame the host sends.
pub fn notify(msg: Message) -> Step {
    Step::Out(msg.encode())
}

/// A frame the device sends.
pub fn reply(msg: Message) -> Step {
    Step::In(msg.encode())
}

pub fn request(command: u32, args: &[u8]) -> Step {
    notify(Message::new(
        Service::Program,
        SUBSYSTEM,
        command,
        args.to_vec(),
    ))
}

/// The device answering `command` successfully.
pub fn response(command: u32, payload: &[u8]) -> Step {
    response_with_status(command, 0, payload)
}

pub fn response_with_status(command: u32, status: u32, payload: &[u8]) -> Step {
    reply(Message::new(
        Service::Program,
        SUBSYSTEM,
        command + 1,
        [&status.to_be_bytes()[..], payload].concat(),
    ))
}

/// The device refusing `command`, carrying its status and nothing else.
pub fn refusal(command: u32, status: u32) -> Step {
    response_with_status(command, status, &[])
}

/// The unsolicited notification the device queues when its contents change.
pub fn changed() -> Step {
    reply(Message::new(
        Service::Program,
        SUBSYSTEM,
        cmd::CHANGED,
        Vec::new(),
    ))
}

pub fn ui_request(command: u32) -> Step {
    notify(Message::new(
        Service::Ui,
        ui::SUBSYSTEM,
        command,
        Vec::new(),
    ))
}

pub fn ui_response(command: u32, status: u32) -> Step {
    reply(Message::new(
        Service::Ui,
        ui::SUBSYSTEM,
        command + 1,
        status.to_be_bytes().to_vec(),
    ))
}

/// The UI bracket a transaction opens with, and the class session inside it, at the
/// partition an Electro 5 keeps `class` in.
pub fn session_open(class: ObjectClass) -> Vec<Step> {
    session_open_at(e5_index(class))
}

/// [`session_open`] at a partition index of any table.
pub fn session_open_at(index: u32) -> Vec<Step> {
    vec![
        ui_request(ui::HELLO),
        ui_response(ui::HELLO, 0),
        request(cmd::SESSION_OPEN, &index.to_be_bytes()),
        response(cmd::SESSION_OPEN, &index.to_be_bytes()),
    ]
}

/// The transaction's close. A released session sends only the `GOODBYE` half.
pub fn session_close() -> Vec<Step> {
    vec![
        request(cmd::SESSION_CLOSE, &[]),
        response(cmd::SESSION_CLOSE, &[]),
        ui_request(ui::GOODBYE),
        ui_response(ui::GOODBYE, 0),
    ]
}

/// A location as a command's arguments spell it.
pub fn slot_args(at: Location) -> Vec<u8> {
    let mut v = Vec::new();
    at.write_to(&mut v);
    v
}

pub fn words(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|w| w.to_be_bytes()).collect()
}

/// A host frame the device did not accept within the write's limit.
pub fn unaccepted(step: Step) -> Step {
    match step {
        Step::Out(frame) => Step::OutTimeout(frame),
        other => panic!("only a host frame can go unaccepted: {other:?}"),
    }
}

/// A host frame the transport failed to send.
pub fn failed(step: Step) -> Step {
    match step {
        Step::Out(frame) => Step::OutError(frame),
        other => panic!("only a host frame can fail to send: {other:?}"),
    }
}
