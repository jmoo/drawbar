//! The emulated instrument as a USB *device*: descriptors, endpoint 0, and the URB
//! semantics that [`EmuDevice`] — a frame-level state machine — does not itself carry.
//!
//! The split of responsibilities:
//!
//! - [`EmuDevice`] answers the vendor protocol: a bulk OUT frame in, bulk IN frames out.
//! - [`Gadget`] answers **USB**: enumeration (descriptors, configuration), the
//!   endpoint-0 vendor identity requests, and the fact that a bulk transfer is a thing
//!   a host *waits on* — an IN with nothing to return, or any bulk transfer on a
//!   stalled instrument, pends until data arrives or the host unlinks it. That pending
//!   is what lets the host side's timeouts and cancels run against the same shapes a
//!   cable produces.

use std::collections::VecDeque;

use nord_emu::EmuDevice;
use nord_usb::transport::{CLASS_VENDOR_SPECIFIC, EP_IN, EP_OUT, PRODUCT_ID_ELECTRO5, VENDOR_ID};

use crate::proto::{Completion, SetupPacket, Submit, DIR_IN, DIR_OUT, EPIPE};

/// What the gadget reports about itself: USB identity, descriptor strings, and the
/// endpoint-0 identity words `nord device info` reads.
///
/// ⚠️ The **shapes** are measured (which requests exist, their widths, that an
/// unrecognised request stalls — confirmed on hardware); the default **values** for
/// `kind`/`firmware`/`build`/`max_transfer` are placeholders. Set them from a real
/// instrument (`nord device info` prints all four) when fidelity matters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GadgetConfig {
    pub vendor_id: u16,
    pub product_id: u16,
    /// Also the BCD source for the descriptor's `bcdDevice`, which carries the same
    /// value as the firmware word.
    pub firmware: u16,
    pub kind: u16,
    pub build: u16,
    pub max_transfer: u32,
    pub manufacturer: String,
    pub product: String,
    pub serial: String,
    /// The bus id `usbip attach -b` names. Also reported in the device list.
    pub busid: String,
    pub busnum: u32,
    pub devnum: u32,
}

impl Default for GadgetConfig {
    fn default() -> Self {
        Self {
            vendor_id: VENDOR_ID,
            product_id: PRODUCT_ID_ELECTRO5,
            firmware: 204,
            kind: 1,
            build: 1,
            max_transfer: 32768,
            manufacturer: "Clavia DMI AB".into(),
            product: "Nord Electro 5".into(),
            // Deliberately not shaped like a real serial: anything reading it should
            // be able to tell this instrument is emulated.
            serial: "nord-emu".into(),
            busid: "1-1".into(),
            busnum: 1,
            devnum: 2,
        }
    }
}

impl GadgetConfig {
    pub fn devid(&self) -> u32 {
        (self.busnum << 16) | self.devnum
    }

    /// The standard 18-byte device descriptor.
    ///
    /// `bcdUSB` 1.10 with a 64-byte endpoint 0: the link is Full Speed.
    pub fn device_descriptor(&self) -> Vec<u8> {
        let mut d = vec![18, 1];
        d.extend_from_slice(&0x0110u16.to_le_bytes());
        d.extend_from_slice(&[0, 0, 0, 64]); // class per interface; ep0 max packet
        d.extend_from_slice(&self.vendor_id.to_le_bytes());
        d.extend_from_slice(&self.product_id.to_le_bytes());
        d.extend_from_slice(&bcd(self.firmware).to_le_bytes());
        d.extend_from_slice(&[1, 2, 3, 1]); // iManufacturer/iProduct/iSerial, 1 config
        d
    }

    /// Configuration + interface + endpoint descriptors, one block.
    ///
    /// ⚠️ One deliberate divergence from hardware: the real instrument also carries a
    /// USB-MIDI (audio-class) interface, which is omitted here so no host MIDI driver
    /// binds to an emulator that has no audio to offer. The vendor interface and its
    /// endpoint addresses are the ones every backend in [`nord_usb`] looks for.
    pub fn configuration_descriptor(&self) -> Vec<u8> {
        let mut c = vec![9, 2];
        c.extend_from_slice(&32u16.to_le_bytes()); // wTotalLength: 9 + 9 + 7 + 7
        c.extend_from_slice(&[1, 1, 0, 0xc0, 0]); // 1 interface, config 1, self-powered
        c.extend_from_slice(&[9, 4, 0, 0, 2, CLASS_VENDOR_SPECIFIC, 0, 0, 0]);
        for ep in [EP_IN, EP_OUT] {
            c.extend_from_slice(&[7, 5, ep, 2]); // bulk
            c.extend_from_slice(&64u16.to_le_bytes());
            c.push(0);
        }
        c
    }

    fn string_descriptor(&self, index: u8) -> Option<Vec<u8>> {
        if index == 0 {
            return Some(vec![4, 3, 0x09, 0x04]); // en-US
        }
        let s = match index {
            1 => &self.manufacturer,
            2 => &self.product,
            3 => &self.serial,
            _ => return None,
        };
        let mut d = vec![(2 + 2 * s.encode_utf16().count()) as u8, 3];
        for unit in s.encode_utf16() {
            d.extend_from_slice(&unit.to_le_bytes());
        }
        Some(d)
    }
}

/// `firmware` as the descriptor's BCD: 204 → `0x0204`, read back as 2.04.
pub(crate) fn bcd(firmware: u16) -> u16 {
    let (major, minor) = (firmware / 100, firmware % 100);
    (major / 10) << 12 | (major % 10) << 8 | (minor / 10) << 4 | (minor % 10)
}

/// A URB the gadget is holding instead of completing.
#[derive(Debug)]
enum Pending {
    /// A bulk IN waiting for the device to have something to say. `cap` bounds the
    /// completion's payload.
    In { seqnum: u32, cap: u32 },
    /// Any bulk transfer submitted while the endpoints are stalled. Never completes:
    /// on hardware the transfer simply does not finish, and only the host's own
    /// cancel (an unlink here, a timeout there) gets it back.
    Parked { seqnum: u32 },
}

impl Pending {
    fn seqnum(&self) -> u32 {
        match self {
            Pending::In { seqnum, .. } | Pending::Parked { seqnum } => *seqnum,
        }
    }
}

/// See the module doc. Owns the [`EmuDevice`] and the queue of held URBs.
pub struct Gadget {
    device: EmuDevice,
    config: GadgetConfig,
    pending: VecDeque<Pending>,
}

impl Gadget {
    pub fn new(device: EmuDevice, config: GadgetConfig) -> Self {
        Self {
            device,
            config,
            pending: VecDeque::new(),
        }
    }

    pub fn config(&self) -> &GadgetConfig {
        &self.config
    }

    pub fn device(&self) -> &EmuDevice {
        &self.device
    }

    pub fn device_mut(&mut self) -> &mut EmuDevice {
        &mut self.device
    }

    /// Handle one `CMD_SUBMIT`. The completions may cover *earlier* submits too: a
    /// bulk OUT that draws replies completes every IN that was pending for them.
    pub fn submit(&mut self, urb: Submit) -> Vec<Completion> {
        match (urb.ep as u8, urb.direction) {
            (0, _) => vec![self.control(&urb)],
            (n, DIR_OUT) if n == EP_OUT & 0x7f => self.bulk_out(urb),
            (n, DIR_IN) if n == EP_IN & 0x7f => self.bulk_in(urb),
            // An endpoint the device does not have.
            _ => vec![refuse(urb.seqnum)],
        }
    }

    /// Handle one `CMD_UNLINK`: the status for the `RET_UNLINK`.
    ///
    /// `-ECONNRESET` when the URB was still held here — the answer that tells the peer
    /// its cancel won the race. `0` when nothing was held, meaning the URB completed
    /// (or never existed); the peer disambiguates by seqnum.
    pub fn unlink(&mut self, target_seqnum: u32) -> i32 {
        let before = self.pending.len();
        self.pending.retain(|p| p.seqnum() != target_seqnum);
        match self.pending.len() < before {
            true => crate::proto::ECONNRESET,
            false => 0,
        }
    }

    fn bulk_out(&mut self, urb: Submit) -> Vec<Completion> {
        if self.device.endpoints_stalled() {
            // ⚠️ Not `-EPIPE`: the observed stall is a write that never completes while
            // the instrument plays on, not a fault the host is told about.
            self.pending
                .push_back(Pending::Parked { seqnum: urb.seqnum });
            return Vec::new();
        }
        // A frame that does not decode is dropped, as hardware drops it: the *USB*
        // transfer still succeeded — the bytes were accepted — and there is no protocol
        // reply to give.
        let _ = self.device.receive(&urb.data);
        let mut done = vec![Completion {
            seqnum: urb.seqnum,
            status: 0,
            actual: urb.data.len() as u32,
            data: Vec::new(),
        }];
        done.extend(self.drain());
        done
    }

    fn bulk_in(&mut self, urb: Submit) -> Vec<Completion> {
        if self.device.endpoints_stalled() {
            self.pending
                .push_back(Pending::Parked { seqnum: urb.seqnum });
            return Vec::new();
        }
        self.pending.push_back(Pending::In {
            seqnum: urb.seqnum,
            cap: urb.length,
        });
        self.drain()
    }

    /// Complete pending INs, oldest first, for as long as the device has replies.
    fn drain(&mut self) -> Vec<Completion> {
        let mut done = Vec::new();
        while matches!(self.pending.front(), Some(Pending::In { .. })) && self.device.has_response()
        {
            let Some(Pending::In { seqnum, cap }) = self.pending.pop_front() else {
                unreachable!()
            };
            let mut data = self.device.take_response().unwrap();
            // A reply larger than the posted buffer cannot happen against nord-usb
            // (it posts 48KB) but must not panic against a smaller host.
            data.truncate(cap as usize);
            done.push(Completion {
                seqnum,
                status: 0,
                actual: data.len() as u32,
                data,
            });
        }
        done
    }

    /// Endpoint 0. Keeps answering while the bulk endpoints are stalled — on hardware
    /// that is exactly the state `nord device info` still works in.
    fn control(&mut self, urb: &Submit) -> Completion {
        let setup = SetupPacket::parse(&urb.setup);
        let reply = self.control_request(setup);
        match reply {
            Some(mut data) => {
                data.truncate(setup.length as usize);
                Completion {
                    seqnum: urb.seqnum,
                    status: 0,
                    actual: data.len() as u32,
                    data: if urb.direction == DIR_IN {
                        data
                    } else {
                        Vec::new()
                    },
                }
            }
            // ⚠️ A request the device does not recognise stalls endpoint 0. Confirmed
            // on hardware for vendor requests (the `nord device controls` sweep).
            None => refuse(urb.seqnum),
        }
    }

    /// `Some(payload)` (possibly empty, for status-stage-only requests) or `None` for
    /// a stall.
    fn control_request(&mut self, s: SetupPacket) -> Option<Vec<u8>> {
        match (s.request_type, s.request) {
            // GET_DESCRIPTOR.
            (0x80, 6) => match (s.value >> 8) as u8 {
                1 => Some(self.config.device_descriptor()),
                2 => Some(self.config.configuration_descriptor()),
                3 => self.config.string_descriptor(s.value as u8),
                // No device qualifier: a Full-Speed-only device stalls the request.
                _ => None,
            },
            // GET_STATUS on the device: self-powered.
            (0x80, 0) => Some(vec![1, 0]),
            // GET_STATUS on an endpoint: the halt bit is the stall.
            (0x82, 0) => Some(vec![u8::from(self.device.endpoints_stalled()), 0]),
            (0x81, 0) => Some(vec![0, 0]),
            // GET_CONFIGURATION / GET_INTERFACE: the only ones there are.
            (0x80, 8) => Some(vec![1]),
            (0x81, 10) => Some(vec![0]),
            // SET_CONFIGURATION / SET_INTERFACE: accepted, nothing to do.
            (0x00, 9) | (0x01, 11) => Some(Vec::new()),
            // CLEAR_FEATURE(ENDPOINT_HALT): accepted, but the stall stays — on hardware
            // it outlives everything short of a power cycle. Whether the real
            // instrument even accepts this request there is unmeasured.
            (0x02, 1) => Some(Vec::new()),
            // The vendor identity words, device recipient, device to host. Shapes
            // confirmed on hardware; values from the config.
            (0xc0, 0x00) => Some(self.config.kind.to_le_bytes().to_vec()),
            (0xc0, 0x04) => Some(self.config.firmware.to_le_bytes().to_vec()),
            (0xc0, 0x05) => Some(self.config.build.to_le_bytes().to_vec()),
            (0xc0, 0x08) => Some(self.config.max_transfer.to_le_bytes().to_vec()),
            _ => None,
        }
    }
}

fn refuse(seqnum: u32) -> Completion {
    Completion {
        seqnum,
        status: EPIPE,
        actual: 0,
        data: Vec::new(),
    }
}
