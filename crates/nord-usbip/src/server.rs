//! The connection loop: OP phase, then URBs until the peer detaches.

use std::io::{self, Read, Write};

use crate::gadget::Gadget;
use crate::proto::{self, Submit};

/// Serve one USB/IP connection to completion.
///
/// A device-list request is answered and the connection ends, per protocol. An import
/// switches the same connection into the URB phase, which runs until the peer detaches
/// (EOF) or the stream fails. The gadget — and with it the emulated instrument's whole
/// state — outlives the connection on purpose: detach and re-attach is a cable pull,
/// not a power cycle.
pub fn handle_connection<S: Read + Write>(stream: &mut S, gadget: &mut Gadget) -> io::Result<()> {
    let mut header = [0u8; 8];
    stream.read_exact(&mut header)?;
    let code = proto::u16_at(&header, 2);

    match code {
        proto::OP_REQ_DEVLIST => {
            stream.write_all(&devlist_reply(gadget))?;
            Ok(())
        }
        proto::OP_REQ_IMPORT => {
            let mut busid = [0u8; 32];
            stream.read_exact(&mut busid)?;
            let asked = str_field(&busid);
            if asked != gadget.config().busid {
                let mut rep = Vec::new();
                proto::put_u16(&mut rep, proto::VERSION);
                proto::put_u16(&mut rep, proto::OP_REP_IMPORT);
                proto::put_u32(&mut rep, proto::ST_NA);
                stream.write_all(&rep)?;
                return Ok(());
            }
            stream.write_all(&import_reply(gadget))?;
            urb_loop(stream, gadget)
        }
        other => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unsupported USB/IP op {other:#06x}"),
        )),
    }
}

fn urb_loop<S: Read + Write>(stream: &mut S, gadget: &mut Gadget) -> io::Result<()> {
    loop {
        let mut header = [0u8; proto::URB_HEADER];
        match stream.read_exact(&mut header) {
            Ok(()) => {}
            // Detach: the peer closes the socket, which is not an error.
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(e) => return Err(e),
        }

        let command = proto::u32_at(&header, 0);
        let seqnum = proto::u32_at(&header, 4);
        match command {
            proto::CMD_SUBMIT => {
                let direction = proto::u32_at(&header, 12);
                let ep = proto::u32_at(&header, 16);
                let length = proto::u32_at(&header, 24);
                let mut setup = [0u8; 8];
                setup.copy_from_slice(&header[40..48]);

                let mut data = Vec::new();
                if direction == proto::DIR_OUT && length > 0 {
                    data.resize(length as usize, 0);
                    stream.read_exact(&mut data)?;
                }

                for completion in gadget.submit(Submit {
                    seqnum,
                    direction,
                    ep,
                    length,
                    setup,
                    data,
                }) {
                    stream.write_all(&completion.encode())?;
                }
            }
            proto::CMD_UNLINK => {
                let target = proto::u32_at(&header, 20);
                let status = gadget.unlink(target);
                stream.write_all(&proto::encode_ret_unlink(seqnum, status))?;
            }
            other => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("unsupported USB/IP command {other:#010x}"),
                ));
            }
        }
    }
}

/// The 312-byte exported-device block shared by both OP replies.
fn device_block(gadget: &Gadget) -> Vec<u8> {
    let c = gadget.config();
    let mut b = Vec::with_capacity(312);
    proto::put_padded(&mut b, &format!("/nord-emu/{}", c.busid), 256);
    proto::put_padded(&mut b, &c.busid, 32);
    proto::put_u32(&mut b, c.busnum);
    proto::put_u32(&mut b, c.devnum);
    proto::put_u32(&mut b, proto::SPEED_FULL);
    proto::put_u16(&mut b, c.vendor_id);
    proto::put_u16(&mut b, c.product_id);
    proto::put_u16(&mut b, crate::gadget::bcd(c.firmware));
    // Class per interface; configuration 1 of 1; one interface.
    b.extend_from_slice(&[0, 0, 0, 1, 1, 1]);
    b
}

fn devlist_reply(gadget: &Gadget) -> Vec<u8> {
    let mut rep = Vec::new();
    proto::put_u16(&mut rep, proto::VERSION);
    proto::put_u16(&mut rep, proto::OP_REP_DEVLIST);
    proto::put_u32(&mut rep, proto::ST_OK);
    proto::put_u32(&mut rep, 1);
    rep.extend_from_slice(&device_block(gadget));
    // The one interface: vendor-specific, plus the pad byte.
    rep.extend_from_slice(&[nord_usb::transport::CLASS_VENDOR_SPECIFIC, 0, 0, 0]);
    rep
}

fn import_reply(gadget: &Gadget) -> Vec<u8> {
    let mut rep = Vec::new();
    proto::put_u16(&mut rep, proto::VERSION);
    proto::put_u16(&mut rep, proto::OP_REP_IMPORT);
    proto::put_u32(&mut rep, proto::ST_OK);
    rep.extend_from_slice(&device_block(gadget));
    rep
}

fn str_field(raw: &[u8]) -> &str {
    let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
    std::str::from_utf8(&raw[..end]).unwrap_or("")
}
