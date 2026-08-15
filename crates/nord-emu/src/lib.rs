//! A device-side emulator for the Clavia / Nord vendor USB protocol.
//!
//! [`nord_usb`] is the host half; this is an instrument to point it at. An
//! [`EmuDevice`] holds emulated storage and answers the protocol from it, and
//! [`EmuTransport`] plugs that into [`nord_usb::Transport`], so sessions, operations and
//! envelopes run against a moving target with no hardware attached.
//!
//! ```no_run
//! use nord_emu::{EmuDevice, EmuTransport, Object};
//! use nord_usb::{op, Location, ObjectClass, Session};
//!
//! # async fn demo() -> nord_usb::Result<()> {
//! let mut device = EmuDevice::new();
//! device.insert(
//!     ObjectClass::Program,
//!     Location::from_user(7, 4),
//!     Object::new("Bright Grand", b"ne5p", 4, vec![0; 121]),
//! );
//!
//! let mut t = EmuTransport::new(device);
//! let mut s = Session::open(&mut t, ObjectClass::Program).await?;
//! let info = op::info(&mut s, Location::from_user(7, 4)).await?;
//! s.commit().await?;
//! assert_eq!(info.name, "Bright Grand");
//! # Ok(()) }
//! ```
//!
//! # Why, given [`nord_usb::ReplayTransport`] exists
//!
//! A replay is a recording: it can only answer what a real device once answered, in the
//! order it answered. That pins the bytes a host emits, which is exactly what it is for.
//! It cannot express anything **stateful** — a delete that makes the next read fail, a
//! move that swaps two slots and rewrites the set lists pointing at them, a reply that
//! arrives out of turn, a device that goes quiet.
//!
//! # What this models, and how well it is known
//!
//! Everything here comes from this project's own captures of its own instrument and from
//! behaviour measured on it. Where a shape has never been observed — what `DELETE`
//! answers for an empty slot, for instance — the emulator answers a documented
//! neighbouring status through [`Unmodeled`] rather than inventing protocol, and says so.
//!
//! Modelled: the session and UI handshakes and both of their wedges, the partition and
//! bank geometry, per-class storage counters, object info, dependency lists with the
//! dangling-reference marker resolved at read time, chunked reads, the write path
//! including its empty-destination precondition, delete, rename, move (a swap, with
//! reference fix-up), copy (an overwrite), select and focus, the enumeration cursor and
//! the mutation that poisons it, unsolicited change notifications, and endpoint stalls.
//!
//! Not modelled: bundles, backups and firmware update; the `(Native)` library views as a
//! second window onto one pool; the startup-sync commands NSM sends that no
//! [`nord_usb::op`] does; the schema-version migration a move inflicts on the set lists
//! it rewrites.

pub mod device;
pub mod store;
pub mod transport;

pub use device::{EmuDevice, Unmodeled};
pub use store::{status, Bank, Dependency, Focus, Object, Partition};
pub use transport::{EmuTransport, Frame, Side};
