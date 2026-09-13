//! Bench probe: does a class accept `BEGIN_WRITE` at all?
//!
//! Sends `BEGIN_WRITE` at an occupied slot with the complete identical-bytes write
//! staged. A refusal is the finding (the status code is printed and the session
//! closes cleanly); an acceptance is completed immediately with the bytes the slot
//! already holds, so even a working write changes nothing. `put`'s delete-first
//! composition is deliberately never used — that is the half with no proven undo on
//! a singleton class.
//!
//!     cargo run -p nord-usb --example write_probe --features blocking -- \
//!         <class> <bank:slot> [send=<body-file>] [record.script]
//!
//! With `send=`, the staged write carries that file's bytes instead of the slot's
//! own — the mutation test. The aftermath compare is then against the sent bytes.

use std::time::Duration;

use nord_usb::wire::cmd;
use nord_usb::{op, Location, Message, ObjectClass, Service, Session};

const LIMIT: Duration = Duration::from_secs(10);

fn report(step: &str, reply: Option<&Message>) -> Option<u32> {
    match reply {
        None => {
            println!("{step}: no reply within {LIMIT:?}");
            None
        }
        Some(msg) => {
            let status = msg.status();
            println!(
                "{step}: reply command {:#x}, status {status:?}",
                msg.command
            );
            status
        }
    }
}

/// `BANK:SLOT` in the panel's one-indexed numbering, as every other entry point spells
/// an address.
fn address(text: &str) -> Location {
    let (bank, slot) = text.split_once(':').expect("BANK:SLOT");
    let bank = bank.parse::<u32>().expect("bank");
    let slot = slot.parse::<u32>().expect("slot");
    assert!(bank >= 1 && slot >= 1, "BANK:SLOT is one-indexed");
    Location::from_user(bank, slot)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let class = ObjectClass::from_raw(args[1].parse::<u32>().expect("class number"));
    let at = address(&args[2]);

    let send: Option<Vec<u8>> = args
        .iter()
        .find_map(|a| a.strip_prefix("send="))
        .map(|p| std::fs::read(p).expect("send body file"));
    let transport = nord_usb::transport::UsbTransport::open_first().expect("open device");
    let mut transport = match args
        .get(3)
        .filter(|a| !a.starts_with("send="))
        .or(args.get(4))
    {
        Some(path) => transport
            .recording_to(std::path::Path::new(path))
            .expect("recording"),
        None => transport,
    };

    nord_usb::block_on(async {
        // Baseline: what the slot holds, read in its own session.
        let (info, before) = {
            let mut s = Session::open(&mut transport, class).await.expect("open");
            let info = op::info(&mut s, at).await.expect("info");
            let body = op::read_body(&mut s, at).await.expect("read body");
            s.commit().await.expect("close");
            (info, body)
        };
        println!(
            "target: {:?} \"{}\" format {} body {} bytes",
            class,
            info.name,
            info.format,
            before.len()
        );

        let payload = send.as_ref().unwrap_or(&before);
        if let Some(p) = &send {
            assert_eq!(
                p.len(),
                before.len(),
                "send body must match the slot's length"
            );
            println!(
                "mutation mode: sending {} bytes that differ from the slot",
                p.len()
            );
        }

        // The probe. Args composed exactly as op::write does, name kept.
        let mut s = Session::open(&mut transport, class)
            .await
            .expect("open")
            .allow_destructive_writes();
        let tag: [u8; 4] = info
            .format
            .as_bytes()
            .try_into()
            .expect("a four-character format tag");
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as u32;
        let begin =
            op::begin_write_args(at, payload.len(), &tag, stamp, &info.name).expect("begin args");

        let reply = s
            .probe(Service::Program, 10, cmd::BEGIN_WRITE, &begin, LIMIT)
            .await
            .expect("transport during BEGIN_WRITE");
        match report("BEGIN_WRITE", reply.as_ref()) {
            Some(0) => {
                println!("FINDING: BEGIN_WRITE ACCEPTED — completing with identical bytes");
                let data = op::write_data_args(at, 0, payload).expect("write data args");
                let reply = s
                    .probe(Service::Program, 10, cmd::WRITE_DATA, &data, LIMIT)
                    .await
                    .expect("transport during WRITE_DATA");
                report("WRITE_DATA", reply.as_ref());
                let mut end = Vec::new();
                at.write_to(&mut end);
                let reply = s
                    .probe(Service::Program, 10, cmd::END_TRANSFER, &end, LIMIT)
                    .await
                    .expect("transport during END_TRANSFER");
                report("END_TRANSFER", reply.as_ref());
            }
            Some(code) => {
                println!("FINDING: BEGIN_WRITE refused, device status {code:#x}");
            }
            None => println!("FINDING: BEGIN_WRITE never answered (session may strand)"),
        }
        s.commit().await.expect("close after probe");

        // Aftermath: the slot must hold exactly what it held before.
        let mut s = Session::open(&mut transport, class).await.expect("reopen");
        let after = op::read_body(&mut s, at).await.expect("re-read");
        s.commit().await.expect("close");
        println!(
            "aftermath: body {} bytes, {}",
            after.len(),
            if after == *payload {
                "IDENTICAL to what was sent (landed)"
            } else if after == before {
                "unchanged — the write was ACCEPTED BUT IGNORED"
            } else {
                "*** neither sent nor original — investigate ***"
            }
        );
    });
}
