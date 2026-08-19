//! The server against a scripted USB/IP peer.
//!
//! Each test writes the byte stream a `vhci_hcd` (or usbip-win) peer would send,
//! runs [`handle_connection`] over it, and decodes the reply stream. The vendor
//! frames crossing the bulk endpoints are the same capture hexes `nord-emu`'s own
//! suites pin, so a green run here means an attaching host sees byte-for-byte what a
//! cable delivered.

use std::io::{self, Read, Write};

use nord_emu::{EmuDevice, Object};
use nord_usbip::{handle_connection, proto, Gadget, GadgetConfig};

/// The session wrapper, both directions, straight from the capture corpus (see
/// `nord-emu/tests/capture.rs`).
const HELLO: &str = "0000001200000006000000010000000006a1";
const HELLO_REPLY: &str = "000000160000000600000001000000010000000044ec";
const OPEN_PROGRAM: &str = "000000160000000c0000000a0000000400000004a218";
const OPEN_PROGRAM_REPLY: &str = "0000001a0000000c0000000a00000005000000000000000467b0";
const CLOSE: &str = "000000120000000c0000000a000000066500";
const CLOSE_REPLY: &str = "000000160000000c0000000a00000007000000000c4e";
const GOODBYE: &str = "0000001200000006000000010000000226e3";
const GOODBYE_REPLY: &str = "0000001600000006000000010000000300000000006f";

fn hex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

/// A pre-scripted peer: the whole client stream up front, replies accumulated.
struct Peer {
    input: io::Cursor<Vec<u8>>,
    output: Vec<u8>,
}

impl Read for Peer {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.input.read(buf)
    }
}

impl Write for Peer {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.output.extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Builds the client-side byte stream, remembering which submits were IN — a reply
/// carries data only for those, and like the real peer the parser matches by seqnum.
#[derive(Default)]
struct Script {
    bytes: Vec<u8>,
    in_seqs: std::collections::HashSet<u32>,
}

impl Script {
    fn op(mut self, code: u16) -> Self {
        proto::put_u16(&mut self.bytes, proto::VERSION);
        proto::put_u16(&mut self.bytes, code);
        proto::put_u32(&mut self.bytes, 0);
        self
    }

    fn import(mut self, busid: &str) -> Self {
        self = self.op(proto::OP_REQ_IMPORT);
        proto::put_padded(&mut self.bytes, busid, 32);
        self
    }

    fn submit(
        mut self,
        seqnum: u32,
        direction: u32,
        ep: u32,
        length: u32,
        setup: [u8; 8],
        data: &[u8],
    ) -> Self {
        if direction == proto::DIR_IN {
            self.in_seqs.insert(seqnum);
        }
        proto::put_u32(&mut self.bytes, proto::CMD_SUBMIT);
        proto::put_u32(&mut self.bytes, seqnum);
        proto::put_u32(&mut self.bytes, 0x0001_0002); // devid, unchecked
        proto::put_u32(&mut self.bytes, direction);
        proto::put_u32(&mut self.bytes, ep);
        proto::put_u32(&mut self.bytes, 0); // transfer_flags
        proto::put_u32(&mut self.bytes, length);
        proto::put_u32(&mut self.bytes, 0); // start_frame
        proto::put_u32(&mut self.bytes, 0xffff_ffff); // number_of_packets: not ISO
        proto::put_u32(&mut self.bytes, 0); // interval
        self.bytes.extend_from_slice(&setup);
        self.bytes.extend_from_slice(data);
        self
    }

    fn control_in(
        self,
        seqnum: u32,
        request_type: u8,
        request: u8,
        value: u16,
        length: u16,
    ) -> Self {
        let mut setup = [0u8; 8];
        setup[0] = request_type;
        setup[1] = request;
        setup[2..4].copy_from_slice(&value.to_le_bytes());
        setup[6..8].copy_from_slice(&length.to_le_bytes());
        self.submit(seqnum, proto::DIR_IN, 0, length as u32, setup, &[])
    }

    fn bulk_out(self, seqnum: u32, frame: &[u8]) -> Self {
        self.submit(seqnum, proto::DIR_OUT, 3, frame.len() as u32, [0; 8], frame)
    }

    fn bulk_in(self, seqnum: u32) -> Self {
        self.submit(seqnum, proto::DIR_IN, 2, 49152, [0; 8], &[])
    }

    fn unlink(mut self, seqnum: u32, target: u32) -> Self {
        proto::put_u32(&mut self.bytes, proto::CMD_UNLINK);
        proto::put_u32(&mut self.bytes, seqnum);
        proto::put_u32(&mut self.bytes, 0x0001_0002);
        proto::put_u32(&mut self.bytes, 0);
        proto::put_u32(&mut self.bytes, 0);
        proto::put_u32(&mut self.bytes, target);
        self.bytes.resize(self.bytes.len() + 24, 0);
        self
    }
}

/// One decoded reply off the server's stream.
#[derive(Debug, PartialEq, Eq)]
enum Reply {
    Submit {
        seqnum: u32,
        status: i32,
        data: Vec<u8>,
    },
    Unlink {
        seqnum: u32,
        status: i32,
    },
}

/// Run a script that begins with an import against a fresh or given device.
fn attach(device: EmuDevice, script: Script) -> (Vec<Reply>, Gadget) {
    let mut gadget = Gadget::new(device, GadgetConfig::default());
    let mut peer = Peer {
        input: io::Cursor::new(script.bytes),
        output: Vec::new(),
    };
    handle_connection(&mut peer, &mut gadget).expect("connection failed");

    // The import reply is fixed-size; everything after is URB replies.
    let out = peer.output;
    assert_eq!(proto::u16_at(&out, 2), proto::OP_REP_IMPORT);
    assert_eq!(proto::u32_at(&out, 4), proto::ST_OK, "import refused");
    let mut at = 8 + 312;
    let mut replies = Vec::new();
    while at < out.len() {
        let command = proto::u32_at(&out, at);
        let seqnum = proto::u32_at(&out, at + 4);
        let status = proto::u32_at(&out, at + 20) as i32;
        match command {
            proto::RET_SUBMIT => {
                // Data follows only for an IN — the peer knows which by seqnum.
                let actual = match script.in_seqs.contains(&seqnum) {
                    true => proto::u32_at(&out, at + 24) as usize,
                    false => 0,
                };
                let data = out[at + 48..at + 48 + actual].to_vec();
                at += 48 + actual;
                replies.push(Reply::Submit {
                    seqnum,
                    status,
                    data,
                });
            }
            proto::RET_UNLINK => {
                at += 48;
                replies.push(Reply::Unlink { seqnum, status });
            }
            other => panic!("unexpected reply command {other:#x} at {at}"),
        }
    }
    (replies, gadget)
}

/// `usbip list -r` sees one Full-Speed device with Clavia's ids and the vendor
/// interface — the same things every `nord-usb` backend keys on.
#[test]
fn the_device_list_reports_the_instrument() {
    let mut gadget = Gadget::new(EmuDevice::new(), GadgetConfig::default());
    let mut peer = Peer {
        input: io::Cursor::new(Script::default().op(proto::OP_REQ_DEVLIST).bytes),
        output: Vec::new(),
    };
    handle_connection(&mut peer, &mut gadget).unwrap();

    let out = peer.output;
    assert_eq!(proto::u16_at(&out, 2), proto::OP_REP_DEVLIST);
    assert_eq!(proto::u32_at(&out, 8), 1, "exactly one device");
    let dev = &out[12..];
    assert_eq!(proto::u32_at(dev, 296), proto::SPEED_FULL);
    assert_eq!(proto::u16_at(dev, 300), 0x0ffc, "Clavia vendor id");
    assert_eq!(proto::u16_at(dev, 302), 0x0027, "Electro 5 product id");
    assert_eq!(dev[311], 1, "one interface");
    assert_eq!(dev[312], 0xff, "vendor-specific class");
    assert_eq!(
        out.len(),
        12 + 312 + 4,
        "one device block, one interface row"
    );
}

/// Enumeration: the device descriptor carries the ids, the configuration exposes the
/// two bulk endpoints at the addresses `nord-usb` hardcodes, and SET_CONFIGURATION is
/// accepted.
#[test]
fn enumeration_answers_the_descriptors() {
    let script = Script::default()
        .import("1-1")
        .control_in(1, 0x80, 6, 1 << 8, 18) // device descriptor
        .control_in(2, 0x80, 6, 2 << 8, 255) // configuration
        .submit(3, proto::DIR_OUT, 0, 0, [0x00, 9, 1, 0, 0, 0, 0, 0], &[]); // SET_CONFIGURATION(1)
    let (replies, _) = attach(EmuDevice::new(), script);

    let Reply::Submit {
        status: 0,
        data: dev,
        ..
    } = &replies[0]
    else {
        panic!("device descriptor refused: {:?}", replies[0]);
    };
    assert_eq!(dev.len(), 18);
    assert_eq!(
        &dev[8..12],
        &[0xfc, 0x0f, 0x27, 0x00],
        "VID/PID little-endian"
    );

    let Reply::Submit {
        status: 0,
        data: config,
        ..
    } = &replies[1]
    else {
        panic!("configuration refused: {:?}", replies[1]);
    };
    assert_eq!(config.len(), 32, "config + interface + 2 endpoints");
    assert_eq!(config[14], 0xff, "vendor-specific interface class");
    let endpoints: Vec<u8> = vec![config[20], config[27]];
    assert_eq!(endpoints, vec![0x82, 0x03], "the addresses nord-usb claims");

    assert_eq!(
        replies[2],
        Reply::Submit {
            seqnum: 3,
            status: 0,
            data: Vec::new()
        }
    );
}

/// The identity words `nord device info` reads over endpoint 0, and the confirmed
/// stall for a request the instrument does not recognise.
#[test]
fn vendor_identity_answers_and_unknown_requests_stall() {
    let script = Script::default()
        .import("1-1")
        .control_in(1, 0xc0, 0x08, 0, 4) // max transfer
        .control_in(2, 0xc0, 0x04, 0, 2) // firmware
        .control_in(3, 0xc0, 0x77, 0, 2); // never observed to answer
    let (replies, gadget) = attach(EmuDevice::new(), script);

    assert_eq!(
        replies[0],
        Reply::Submit {
            seqnum: 1,
            status: 0,
            data: gadget.config().max_transfer.to_le_bytes().to_vec()
        }
    );
    assert_eq!(
        replies[1],
        Reply::Submit {
            seqnum: 2,
            status: 0,
            data: gadget.config().firmware.to_le_bytes().to_vec()
        }
    );
    assert_eq!(
        replies[2],
        Reply::Submit {
            seqnum: 3,
            status: proto::EPIPE,
            data: Vec::new()
        }
    );
}

/// A whole vendor-protocol transaction over bulk URBs: every reply byte-for-byte what
/// the capture holds. This is the exchange every attaching host runs first.
#[test]
fn a_session_over_urbs_reproduces_the_capture() {
    let script = Script::default()
        .import("1-1")
        .bulk_out(1, &hex(HELLO))
        .bulk_in(2)
        .bulk_out(3, &hex(OPEN_PROGRAM))
        .bulk_in(4)
        .bulk_out(5, &hex(CLOSE))
        .bulk_in(6)
        .bulk_out(7, &hex(GOODBYE))
        .bulk_in(8);
    let (replies, _) = attach(EmuDevice::new(), script);

    let expected = [
        (2u32, HELLO_REPLY),
        (4, OPEN_PROGRAM_REPLY),
        (6, CLOSE_REPLY),
        (8, GOODBYE_REPLY),
    ];
    let ins: Vec<&Reply> = replies
        .iter()
        .filter(|r| matches!(r, Reply::Submit { data, .. } if !data.is_empty()))
        .collect();
    assert_eq!(ins.len(), expected.len());
    for (reply, (seqnum, hexes)) in ins.iter().zip(expected) {
        assert_eq!(
            **reply,
            Reply::Submit {
                seqnum,
                status: 0,
                data: hex(hexes)
            }
        );
    }
}

/// An IN posted before the device has anything to say pends — like a URB on a real
/// bus — and completes the moment an OUT draws the reply. The posted-ahead read is
/// how NSM actually drives the endpoint.
#[test]
fn an_early_bulk_in_pends_until_the_reply_exists() {
    let script = Script::default()
        .import("1-1")
        .bulk_in(1)
        .bulk_out(2, &hex(HELLO));
    let (replies, _) = attach(EmuDevice::new(), script);

    // The OUT completes first (it finished on the spot); the IN completes on its
    // heels with the reply the OUT produced.
    assert_eq!(
        replies,
        vec![
            Reply::Submit {
                seqnum: 2,
                status: 0,
                data: Vec::new()
            },
            Reply::Submit {
                seqnum: 1,
                status: 0,
                data: hex(HELLO_REPLY)
            },
        ]
    );
}

/// A pending IN the host gives up on is unlinked, not completed: the cancel path the
/// host side's `read_timeout` takes on a real bus.
#[test]
fn an_unlinked_pending_in_answers_econnreset() {
    let script = Script::default().import("1-1").bulk_in(7).unlink(8, 7);
    let (replies, _) = attach(EmuDevice::new(), script);

    assert_eq!(
        replies,
        vec![Reply::Unlink {
            seqnum: 8,
            status: proto::ECONNRESET
        }],
        "the URB must be unlinked, never completed"
    );
}

/// Unlinking a URB that already completed answers 0 — the race the protocol defines.
#[test]
fn an_unlink_after_completion_answers_zero() {
    let script = Script::default()
        .import("1-1")
        .bulk_out(1, &hex(HELLO))
        .bulk_in(2)
        .unlink(3, 2);
    let (replies, _) = attach(EmuDevice::new(), script);

    assert_eq!(
        replies.last(),
        Some(&Reply::Unlink {
            seqnum: 3,
            status: 0
        })
    );
}

/// On a stalled instrument, bulk transfers in both directions hang — parked here,
/// never completed — while endpoint 0 keeps answering, which is exactly the state
/// `nord device info` still works in. Confirmed on hardware.
#[test]
fn a_stalled_instrument_parks_bulk_and_answers_control() {
    let mut device = EmuDevice::new();
    device.insert(
        nord_usb::ObjectClass::Program,
        nord_usb::Location::from_user(1, 1),
        Object::new("Held", b"ne5p", 4, vec![0; 121]),
    );
    device.stall_endpoints();

    let script = Script::default()
        .import("1-1")
        .bulk_out(1, &hex(HELLO))
        .bulk_in(2)
        .control_in(3, 0xc0, 0x04, 0, 2) // identity still answers
        .unlink(4, 1)
        .unlink(5, 2);
    let (replies, gadget) = attach(device, script);

    assert_eq!(
        replies,
        vec![
            Reply::Submit {
                seqnum: 3,
                status: 0,
                data: gadget.config().firmware.to_le_bytes().to_vec()
            },
            Reply::Unlink {
                seqnum: 4,
                status: proto::ECONNRESET
            },
            Reply::Unlink {
                seqnum: 5,
                status: proto::ECONNRESET
            },
        ]
    );
}

/// A wrong bus id is refused with a non-zero status and no device block.
#[test]
fn importing_an_unknown_busid_is_refused() {
    let mut gadget = Gadget::new(EmuDevice::new(), GadgetConfig::default());
    let mut peer = Peer {
        input: io::Cursor::new(Script::default().import("3-7").bytes),
        output: Vec::new(),
    };
    handle_connection(&mut peer, &mut gadget).unwrap();
    assert_eq!(proto::u16_at(&peer.output, 2), proto::OP_REP_IMPORT);
    assert_eq!(proto::u32_at(&peer.output, 4), proto::ST_NA);
    assert_eq!(peer.output.len(), 8, "no device block after a refusal");
}
