//! The USB/IP wire protocol, as `vhci_hcd` and usbip-win speak it.
//!
//! Two phases share one TCP connection: an **OP** phase of 8-byte-headed request/reply
//! pairs (device list, import), then — once an import succeeds — a **URB** phase of
//! 48-byte-headed commands carrying USB transfers. OP and URB headers are big-endian;
//! the 8-byte USB setup packet keeps its own little-endian USB layout untouched.
//!
//! Reference: `Documentation/usb/usbip_protocol.rst` in the Linux kernel tree.

/// Protocol version both the Linux tool and usbip-win send.
pub const VERSION: u16 = 0x0111;

pub const OP_REQ_DEVLIST: u16 = 0x8005;
pub const OP_REP_DEVLIST: u16 = 0x0005;
pub const OP_REQ_IMPORT: u16 = 0x8003;
pub const OP_REP_IMPORT: u16 = 0x0003;

/// OP status word: success.
pub const ST_OK: u32 = 0;
/// OP status word: refused (unknown bus id, nothing exportable).
pub const ST_NA: u32 = 1;

pub const CMD_SUBMIT: u32 = 0x0000_0001;
pub const CMD_UNLINK: u32 = 0x0000_0002;
pub const RET_SUBMIT: u32 = 0x0000_0003;
pub const RET_UNLINK: u32 = 0x0000_0004;

pub const DIR_OUT: u32 = 0;
pub const DIR_IN: u32 = 1;

/// `usb_device_speed`: the instrument's link is Full Speed.
pub const SPEED_FULL: u32 = 2;

/// URB status for a stalled endpoint: `-EPIPE`.
pub const EPIPE: i32 = -32;
/// URB status for a transfer killed by `CMD_UNLINK`: `-ECONNRESET`.
pub const ECONNRESET: i32 = -104;

/// Every URB-phase message is this long before any transfer data.
pub const URB_HEADER: usize = 48;

pub fn put_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_be_bytes());
}

pub fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_be_bytes());
}

pub fn u16_at(buf: &[u8], at: usize) -> u16 {
    u16::from_be_bytes(buf[at..at + 2].try_into().unwrap())
}

pub fn u32_at(buf: &[u8], at: usize) -> u32 {
    u32::from_be_bytes(buf[at..at + 4].try_into().unwrap())
}

/// A fixed-width NUL-padded string field (`path`, `busid`).
pub fn put_padded(out: &mut Vec<u8>, s: &str, width: usize) {
    let bytes = s.as_bytes();
    let take = bytes.len().min(width);
    out.extend_from_slice(&bytes[..take]);
    out.resize(out.len() + (width - take), 0);
}

/// One parsed `USBIP_CMD_SUBMIT`, transfer data included.
#[derive(Debug, Clone)]
pub struct Submit {
    pub seqnum: u32,
    pub direction: u32,
    /// Endpoint **number** — direction is carried separately, so bulk IN is `ep: 2`,
    /// not `0x82`.
    pub ep: u32,
    /// `transfer_buffer_length`: how much an IN may return, how much an OUT carries.
    pub length: u32,
    /// The raw 8-byte USB setup packet; all zeros on non-control endpoints.
    pub setup: [u8; 8],
    /// OUT payload, empty for IN.
    pub data: Vec<u8>,
}

/// One completed URB, ready to be written as `USBIP_RET_SUBMIT`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completion {
    pub seqnum: u32,
    /// 0 or a negative errno ([`EPIPE`], [`ECONNRESET`]).
    pub status: i32,
    /// `actual_length`. For an OUT this is the byte count accepted, so it is not
    /// always `data.len()`.
    pub actual: u32,
    /// IN payload, empty for OUT.
    pub data: Vec<u8>,
}

impl Completion {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(URB_HEADER + self.data.len());
        put_u32(&mut out, RET_SUBMIT);
        put_u32(&mut out, self.seqnum);
        // devid, direction and ep are unused in replies; the peer matches on seqnum.
        put_u32(&mut out, 0);
        put_u32(&mut out, 0);
        put_u32(&mut out, 0);
        put_u32(&mut out, self.status as u32);
        put_u32(&mut out, self.actual);
        put_u32(&mut out, 0); // start_frame: not isochronous
        put_u32(&mut out, 0); // number_of_packets: not isochronous
        put_u32(&mut out, 0); // error_count
        out.resize(URB_HEADER, 0);
        out.extend_from_slice(&self.data);
        out
    }
}

pub fn encode_ret_unlink(seqnum: u32, status: i32) -> Vec<u8> {
    let mut out = Vec::with_capacity(URB_HEADER);
    put_u32(&mut out, RET_UNLINK);
    put_u32(&mut out, seqnum);
    put_u32(&mut out, 0);
    put_u32(&mut out, 0);
    put_u32(&mut out, 0);
    put_u32(&mut out, status as u32);
    out.resize(URB_HEADER, 0);
    out
}

/// The 8-byte USB setup packet. USB byte order (little-endian words), unlike
/// everything around it.
#[derive(Debug, Clone, Copy)]
pub struct SetupPacket {
    pub request_type: u8,
    pub request: u8,
    pub value: u16,
    pub index: u16,
    pub length: u16,
}

impl SetupPacket {
    pub fn parse(raw: &[u8; 8]) -> Self {
        Self {
            request_type: raw[0],
            request: raw[1],
            value: u16::from_le_bytes([raw[2], raw[3]]),
            index: u16::from_le_bytes([raw[4], raw[5]]),
            length: u16::from_le_bytes([raw[6], raw[7]]),
        }
    }
}
