//! Shared helpers for the emulator suites.
//!
//! Each test binary compiles its own copy and uses part of it.
#![allow(dead_code)]

use nord_emu::{EmuDevice, EmuTransport, Object, Partition};
use nord_usb::wire::{Location, ObjectClass};

pub fn hex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

pub fn hexes(frames: &[&str]) -> Vec<Vec<u8>> {
    frames.iter().map(|f| hex(f)).collect()
}

/// Compare a transport's traffic against a capture, frame by frame, in hex — a byte
/// vector's assertion output is unreadable and the offending frame is the whole point.
pub fn assert_frames(what: &str, got: &[&[u8]], want: &[Vec<u8>]) {
    let show = |f: &[u8]| f.iter().map(|b| format!("{b:02x}")).collect::<String>();
    let got: Vec<String> = got.iter().map(|f| show(f)).collect();
    let want: Vec<String> = want.iter().map(|f| show(f)).collect();
    assert_eq!(got, want, "{what} differs from the capture");
}

/// The one-and-only object the `read_prog_bank8_loc14` capture is about: 121 bytes at
/// panel 8:14, named by NSM's own write of it.
pub const PROGRAM_BODY: &str = "000401df06781fc60000000000000000000000000000000000000100000000000000000000400000000000000002200000000000022000400000008888000008008888000008000000000080000000000080000000000000000000800000000800800000000800020010060401020408140010000000000000";

/// An Electro 5 with that program in the slot the capture read.
pub fn device_with_capture_program() -> EmuDevice {
    let mut device = EmuDevice::new();
    device.insert(
        ObjectClass::Program,
        Location::from_user(8, 14),
        Object::new("1000000000000000", b"ne5p", 4, hex(PROGRAM_BODY)),
    );
    device
}

pub fn transport(device: EmuDevice) -> EmuTransport {
    EmuTransport::new(device)
}

/// Fill a partition with `count` objects costing `used` blocks between them.
///
/// Only the sum is meaningful: pianos and samples genuinely differ in size per item, so
/// the split across them here is arbitrary.
pub fn fill(p: &mut Partition, count: u32, used: u32) {
    let each = used / count;
    let per_bank = p.banks.first().map_or(50, |b| b.slots);
    for i in 0..count {
        let blocks = match i + 1 == count {
            true => used - each * (count - 1),
            false => each,
        };
        let at = Location {
            bank: i / per_bank,
            slot: i % per_bank,
        };
        p.insert(
            at,
            Object::new(&format!("item {i}"), b"ne5p", 4, Vec::new()).with_blocks(blocks),
        );
    }
}
