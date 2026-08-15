//! The emulator against real captures, byte for byte.
//!
//! Each test drives the host code in [`nord_usb`] and compares **both** directions
//! against traffic taken off the wire: the host's messages, which a replay also pins,
//! and the device's answers, which a replay cannot — it supplies those itself.
//!
//! An emulator that only satisfies its own encoder is worth nothing, so every reply
//! asserted here is one a real Nord Electro 5 sent.

mod common;

use common::*;
use nord_emu::{Dependency, EmuDevice, EmuTransport, Object};
use nord_usb::block_on;
use nord_usb::wire::{Location, ObjectClass};
use nord_usb::{op, Session};

/// The transaction wrapper every operation runs inside, both directions.
const HELLO: &str = "0000001200000006000000010000000006a1";
const HELLO_REPLY: &str = "000000160000000600000001000000010000000044ec";
const OPEN_PROGRAM: &str = "000000160000000c0000000a0000000400000004a218";
const OPEN_PROGRAM_REPLY: &str = "0000001a0000000c0000000a00000005000000000000000467b0";
const CLOSE: &str = "000000120000000c0000000a000000066500";
const CLOSE_REPLY: &str = "000000160000000c0000000a00000007000000000c4e";
const GOODBYE: &str = "0000001200000006000000010000000226e3";
const GOODBYE_REPLY: &str = "0000001600000006000000010000000300000000006f";

fn wrap_sent(middle: &[&str]) -> Vec<Vec<u8>> {
    let mut all = vec![HELLO, OPEN_PROGRAM];
    all.extend_from_slice(middle);
    all.extend_from_slice(&[CLOSE, GOODBYE]);
    hexes(&all)
}

fn wrap_received(middle: &[&str]) -> Vec<Vec<u8>> {
    let mut all = vec![HELLO_REPLY, OPEN_PROGRAM_REPLY];
    all.extend_from_slice(middle);
    all.extend_from_slice(&[CLOSE_REPLY, GOODBYE_REPLY]);
    hexes(&all)
}

/// A program-class session that does nothing, against every framing byte of the wrapper.
#[test]
fn the_session_wrapper_matches_the_capture() {
    let mut t = transport(EmuDevice::new());
    block_on(async {
        let s = Session::open(&mut t, ObjectClass::Program).await.unwrap();
        s.commit().await.unwrap();
    });
    assert_frames("the host's wrapper", &t.sent(), &wrap_sent(&[]));
    assert_frames("the device's wrapper", &t.received(), &wrap_received(&[]));
}

/// `read_prog_bank8_loc14`, end to end: the host's messages, the device's answers, and
/// the `.ne5p` NSM itself saved for the slot.
///
/// The `INFO` reply is the load-bearing one — every field of the richest response on the
/// wire, at the offsets a real device puts them, including the `0xffffffff` pair, the
/// name length rather than a scan, and the body's own CRC-32.
#[test]
fn a_program_read_matches_the_capture_in_both_directions() {
    let at = Location::from_user(8, 14);
    let mut t = transport(device_with_capture_program());

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

    assert_frames(
        "the host's read",
        &t.sent(),
        &wrap_sent(&[
            "0000001a0000000c0000000a0000001e000000070000000dc608",
            "000000250000000600000001000000060000000000000c55706c6f6164696e672e2e2ee94e",
            "0000001a0000000c0000000a0000000c000000070000000d5391",
            "000000220000000c0000000a00000012000000070000000d0000000000000079d476",
            "0000001600000006000000010000000700010064927b",
            "0000001a0000000c0000000a0000000e000000070000000d95f6",
        ]),
    );
    assert_frames(
        "the device's read",
        &t.received(),
        &wrap_received(&[
            "000000520000000c0000000a0000001f00000000000000070000000d000000796e65357000000004ffffffffffffffff00000010313030303030303030303030303030300000000000000000a5465db65db1",
            "0000001e0000000c0000000a0000000d00000000000000070000000dc4d4",
            "0000009f0000000c0000000a0000001300000000000000070000000d0000000000000079000401df06781fc60000000000000000000000000000000000000100000000000000000000400000000000000002200000000000022000400000008888000008008888000008000000000080000000000080000000000000000000800000000800800000000800020010060401020408140010000000000000d24c",
            "0000001e0000000c0000000a0000000f00000000000000070000000d4e12",
        ]),
    );

    let expected = hex(
        "4342494e010000006e65357007000d00ffffffff04000000b65d46a500000000000000000000000000000000",
    );
    assert_eq!(
        file,
        [expected, hex(PROGRAM_BODY)].concat(),
        "reconstructed .ne5p differs from the file NSM saved"
    );
}

/// The read-only inventory sweep, replayed against the emulator instead of a script.
///
/// The fixture is `nord-usb`'s own golden capture, and it is used here from the other
/// side: rather than feeding those `I` lines to the host, the emulator has to *produce*
/// them from its counters. Every `O` line has to come back out of the host unchanged too,
/// so this pins the whole four-transaction sweep in both directions at once.
#[test]
fn the_inventory_sweep_reproduces_the_golden_script() {
    const SCRIPT: &str = include_str!("../../nord-usb/tests/fixtures/inventory.script");

    let mut sent = Vec::new();
    let mut received = Vec::new();
    for line in SCRIPT.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (tag, bytes) = line.split_once(' ').expect("'<O|I> <hex>'");
        match tag {
            "O" => sent.push(hex(bytes)),
            _ => received.push(hex(bytes)),
        }
    }

    let mut device = EmuDevice::new();
    for (class, count, used) in [
        (ObjectClass::Piano, 29, 4012),
        (ObjectClass::Sample, 139, 2039),
        (ObjectClass::Program, 380, 53580),
        (ObjectClass::SetList, 63, 2394),
    ] {
        let p = device.partition_mut(class).unwrap();
        // The script was written before the two trailing `STATUS` words were known and
        // carries zeros for them; a real Electro 5 answers 73/2 for pianos and 8/1 for
        // samples. Zeroed here so the fixture is compared to itself rather than to a
        // later measurement.
        p.extra_counters = [0; 2];
        fill(p, count, used);
    }

    let mut t = transport(device);
    let statuses = block_on(op::inventory(&mut t)).unwrap();

    assert_eq!(statuses.len(), 4);
    assert_eq!(statuses[2].count, 380);
    assert_eq!(statuses[2].slots(), Some(400));
    assert_frames("the host's sweep", &t.sent(), &sent);
    assert_frames("the device's sweep", &t.received(), &received);
}

/// The dependency read from `duplicate_prog_7-2_to_7-3`, byte for byte.
///
/// Two library rows: a piano whose section is not routed (flag 0) and a sample that is.
/// Both address by content id and neither has a location, which is what puts
/// `0xffffffff` in the address words rather than zeros.
#[test]
fn a_dependency_list_matches_the_capture() {
    let at = Location::from_user(7, 3);
    let mut device = EmuDevice::new();
    device.insert(
        ObjectClass::Program,
        at,
        Object::new("", b"ne5p", 4, Vec::new()).with_dependencies(vec![
            Dependency::library(
                ObjectClass::Piano,
                0xd303_b5f2,
                "Royal Grand 3D YaS6 XL 5.4",
                false,
            ),
            Dependency::library(ObjectClass::Sample, 0xf2f5_cadc, "africa_split", true),
        ]),
    );

    let mut t = transport(device);
    let deps = block_on(async {
        let mut s = Session::open(&mut t, ObjectClass::Program).await.unwrap();
        let deps = op::dependencies(&mut s, at).await.unwrap();
        s.commit().await.unwrap();
        deps
    });

    assert_eq!(deps.len(), 2);
    assert!(
        !deps[0].is_required(),
        "an unrouted section is not required"
    );
    assert!(deps[1].is_required());
    assert_frames(
        "the host's dependency read",
        &t.sent(),
        &wrap_sent(&["0000001a0000000c0000000a000000280000000600000002333c"]),
    );
    assert_frames(
        "the device's dependency list",
        &t.received(),
        &wrap_received(&["000000820000000c0000000a0000002900000000000000060000000200000002000000000000000001d303b5f20000001a526f79616c204772616e64203344205961533620584c20352e3400000000ffffffffffffffff010000000000000003f2f5cadc0000000c6166726963615f73706c697400000000ffffffffffffffffc791"]),
    );
}

/// The slot-organization ops against their captures: request bytes NSM sent, reply bytes
/// the instrument sent back, and the storage change each one made.
#[test]
fn the_organization_ops_match_their_captures() {
    // `delete_prog_bank7_loc50`: the fire-and-forget label, then the delete.
    let at = Location::from_user(7, 50);
    let mut device = EmuDevice::new();
    device.insert(
        ObjectClass::Program,
        at,
        Object::new("doomed", b"ne5p", 4, Vec::new()),
    );
    let mut t = transport(device);
    block_on(async {
        let mut s = Session::open(&mut t, ObjectClass::Program)
            .await
            .unwrap()
            .allow_destructive_writes();
        op::delete(&mut s, at).await.unwrap();
        s.commit().await.unwrap();
    });
    assert_frames(
        "the host's delete",
        &t.sent(),
        &wrap_sent(&[
            "000000240000000600000001000000060000000000000b44656c6574696e672e2e2e7394",
            "0000001a0000000c0000000a000000140000000600000031741e",
        ]),
    );
    assert_frames(
        "the device's delete",
        &t.received(),
        &wrap_received(&["0000001e0000000c0000000a0000001500000000000000060000003184b4"]),
    );
    assert!(
        t.device().get(ObjectClass::Program, at).is_none(),
        "the slot still holds something after a delete"
    );

    // `rename_prog_6-13`: the reply carries the address only, never the name.
    let at = Location::from_user(6, 13);
    let mut device = EmuDevice::new();
    device.insert(
        ObjectClass::Program,
        at,
        Object::new("before", b"ne5p", 4, Vec::new()),
    );
    let mut t = transport(device);
    block_on(async {
        let mut s = Session::open(&mut t, ObjectClass::Program)
            .await
            .unwrap()
            .allow_destructive_writes();
        op::rename(&mut s, at, "foo").await.unwrap();
        s.commit().await.unwrap();
    });
    assert_frames(
        "the host's rename",
        &t.sent(),
        &wrap_sent(&["000000210000000c0000000a0000001c000000050000000c00000003666f6f0d53"]),
    );
    assert_frames(
        "the device's rename",
        &t.received(),
        &wrap_received(&["0000001e0000000c0000000a0000001d00000000000000050000000c86c2"]),
    );
    assert_eq!(
        t.device().get(ObjectClass::Program, at).unwrap().name,
        "foo"
    );

    // `duplicate_prog_7-2_to_7-3`: one COPY, and the device copies internally.
    let (from, to) = (Location::from_user(7, 2), Location::from_user(7, 3));
    let mut device = EmuDevice::new();
    device.insert(
        ObjectClass::Program,
        from,
        Object::new("original", b"ne5p", 4, vec![7; 121]),
    );
    let mut t = transport(device);
    block_on(async {
        let mut s = Session::open(&mut t, ObjectClass::Program)
            .await
            .unwrap()
            .allow_destructive_writes();
        op::duplicate(&mut s, from, to).await.unwrap();
        s.commit().await.unwrap();
    });
    assert_frames(
        "the host's duplicate",
        &t.sent(),
        &wrap_sent(&["000000220000000c0000000a000000160000000600000001000000060000000265f4"]),
    );
    assert_frames(
        "the device's duplicate",
        &t.received(),
        &wrap_received(&[
            "000000260000000c0000000a000000170000000000000006000000010000000600000002a86a",
        ]),
    );
    let d = t.device();
    assert_eq!(
        d.get(ObjectClass::Program, from),
        d.get(ObjectClass::Program, to),
        "a duplicate is a deep copy, and the source is untouched"
    );

    // `open_on_device_2-12`: SELECT, the inverted-parity command (`0x2f` -> `0x30`).
    let at = Location::from_user(2, 12);
    let mut device = EmuDevice::new();
    device.insert(
        ObjectClass::Program,
        at,
        Object::new("loaded", b"ne5p", 4, Vec::new()),
    );
    let mut t = transport(device);
    block_on(async {
        let mut s = Session::open(&mut t, ObjectClass::Program).await.unwrap();
        op::select(&mut s, at).await.unwrap();
        s.commit().await.unwrap();
    });
    assert_frames(
        "the host's select",
        &t.sent(),
        &wrap_sent(&["0000001a0000000c0000000a0000002f000000010000000b746a"]),
    );
    assert_frames(
        "the device's select",
        &t.received(),
        &wrap_received(&["0000001e0000000c0000000a0000003000000000000000010000000b19df"]),
    );
}

/// `move_prog_8-13_to_7-16`, and the swap the captures cannot show.
///
/// The capture pins the two frames; the emulator adds what happens to storage, which is
/// the part a replay has nothing to say about — an occupied destination is swapped, not
/// overwritten, and the occupant lands in the source slot byte-identical.
#[test]
fn a_move_matches_the_capture_and_swaps_the_occupant() {
    let (from, to) = (Location::from_user(8, 13), Location::from_user(7, 16));
    let mut device = EmuDevice::new();
    let moved = Object::new("moved", b"ne5p", 4, vec![1; 121]);
    let displaced = Object::new("displaced", b"ne5p", 4, vec![2; 121]);
    device.insert(ObjectClass::Program, from, moved.clone());
    device.insert(ObjectClass::Program, to, displaced.clone());

    let mut t = transport(device);
    block_on(async {
        let mut s = Session::open(&mut t, ObjectClass::Program)
            .await
            .unwrap()
            .allow_destructive_writes();
        op::move_object(&mut s, from, to).await.unwrap();
        s.commit().await.unwrap();
    });

    assert_frames(
        "the host's move",
        &t.sent(),
        &wrap_sent(&["000000220000000c0000000a00000018000000070000000c000000060000000f4a55"]),
    );
    assert_frames(
        "the device's move",
        &t.received(),
        &wrap_received(&[
            "000000260000000c0000000a0000001900000000000000070000000c000000060000000f7197",
        ]),
    );
    let d = t.device();
    assert_eq!(d.get(ObjectClass::Program, to), Some(&moved));
    assert_eq!(
        d.get(ObjectClass::Program, from),
        Some(&displaced),
        "the destination's occupant must survive, in the source slot"
    );
}

/// `write_prog_bank7_loc50`: the request `op::write_program` builds and the three replies
/// the instrument gave it.
///
/// The body here is our own — the capture's belongs to the corpus — but every frame
/// compared is framing: `BEGIN_WRITE` carries only the length, the format tag, the
/// timestamp and the name, and all three replies carry the address and nothing else.
/// That last point is the one worth pinning: the reply to a write does **not** echo what
/// it was given.
#[test]
fn a_program_write_matches_the_captured_framing() {
    let at = Location::from_user(7, 50);
    let body = vec![0u8; 121];
    let file = nord_usb::envelope::wrap("ne5p", at, 4, &body).unwrap();

    let mut t = transport(EmuDevice::new());
    block_on(async {
        let mut s = Session::open(&mut t, ObjectClass::Program)
            .await
            .unwrap()
            .allow_destructive_writes();
        // The mtime NSM sent with this very write.
        op::write_program(&mut s, at, &file, 0x6a64_d352)
            .await
            .unwrap();
        s.commit().await.unwrap();
    });

    let sent = t.sent();
    let received = t.received();
    let want_begin = hex("0000002f0000000c0000000a0000000a0000000600000031000000796e6535706a64d352ffffffff000000013027fe");
    assert_frames("the host's BEGIN_WRITE", &sent[3..4], &[want_begin]);
    assert_frames(
        "the device's write replies",
        &received[2..5],
        &hexes(&[
            "0000001e0000000c0000000a0000000b0000000000000006000000311631",
            "0000001e0000000c0000000a000000110000000000000006000000318119",
            "0000001e0000000c0000000a0000000f000000000000000600000031139c",
        ]),
    );

    // And the slot now holds what was written, named `"0"` — the length-prefixed byte in
    // BEGIN_WRITE really is the name, which is why restoring a program is put + rename.
    let stored = t.device().get(ObjectClass::Program, at).unwrap();
    assert_eq!(stored.name, "0");
    assert_eq!(stored.body, body);
    assert_eq!(stored.version, 4);
}

/// The device's own geometry, as it reports it.
///
/// Piano banks are the panel's categories, which is the whole reason a piano address is
/// category:position; the `(Native)` views report a sentinel instead of a capacity.
#[test]
fn the_partition_and_bank_tables_match_the_capture() {
    let mut t = EmuTransport::default();
    let (parts, banks, piano_banks, native) = block_on(async {
        let mut s = Session::open(&mut t, ObjectClass::Program).await.unwrap();
        let parts = op::partitions(&mut s).await.unwrap();
        let banks = op::banks(&mut s, ObjectClass::Program.to_raw())
            .await
            .unwrap();
        let piano = op::banks(&mut s, ObjectClass::Piano.to_raw())
            .await
            .unwrap();
        let native = op::banks(&mut s, 0).await.unwrap();
        s.commit().await.unwrap();
        (parts, banks, piano, native)
    });

    let names: Vec<&str> = parts.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "Piano (Native)",
            "Piano",
            "Samp Lib (Native)",
            "Samp Lib",
            "Program",
            "Set List",
            "Live",
            "Settings",
        ]
    );
    assert!(parts[0].native && !parts[1].native);
    assert!(
        !native[0].is_bounded(),
        "a (Native) view reports the sentinel, not a capacity"
    );

    assert_eq!(banks.len(), 8);
    assert!(banks.iter().all(|b| b.slots == 50));
    let piano: Vec<&str> = piano_banks.iter().map(|b| b.name.as_str()).collect();
    assert_eq!(
        piano,
        ["Grand", "Upright", "EPiano1", "EPiano2", "Clavinet", "Harps"]
    );
}
