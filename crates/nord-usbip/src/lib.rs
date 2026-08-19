//! A USB/IP server around [`nord_emu`]: the emulated instrument on a real USB stack.
//!
//! [`nord_emu`] models the device side of the vendor protocol in-process, which reaches
//! everything built on [`nord_usb::Transport`] — and nothing else. This crate puts the
//! same model behind the USB/IP protocol, so an unmodified host attaches it as an
//! actual USB device:
//!
//! - Linux: `modprobe vhci-hcd`, then `usbip attach -r <host> -b 1-1` — the emulated
//!   instrument enumerates like hardware, and `nord-cli` or NSM under Wine drives it
//!   through the ordinary USB stack.
//! - Windows: usbip-win attaches the same server, which is the road to real Nord Sound
//!   Manager against the emulator.
//!
//! Layering: [`proto`] speaks the USB/IP wire format, [`gadget`] is the USB device —
//! descriptors, endpoint 0 (the vendor identity words included), and the pend/unlink
//! semantics of bulk transfers — and [`server`] runs a connection. The instrument's
//! behavior stays entirely in [`nord_emu`]; nothing here interprets a vendor-protocol
//! frame.
//!
//! What a bus adds that the in-process transport cannot: a silent device leaves the
//! host's URB pending until *its* timeout cancels it, and a stalled instrument hangs
//! writes exactly the way `nord-usb`'s `WRITE_LIMIT` exists to escape. Both shapes are
//! modelled here as held URBs, released only by `CMD_UNLINK`.
//!
//! Not affiliated with, authorized, or endorsed by Clavia DMI AB.

pub mod gadget;
pub mod proto;
pub mod server;

pub use gadget::{Gadget, GadgetConfig};
pub use server::handle_connection;
