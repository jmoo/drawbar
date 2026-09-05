//! An instrument as a value: a transport, what the USB descriptor said it is, and the
//! session bracket every operation has to run inside.
//!
//! [`op`] is the vocabulary — one capture-pinned function per protocol operation — and
//! [`Session`] the transaction they run in. This is where they compose:
//!
//! - [`Device::read`] and [`Device::destructive`] open a transaction, hand the chain the
//!   raw [`Session`], and close it on the failing path as well as the succeeding one.
//! - [`Geometry`] is the instrument's own partition and bank tables, so what bounds a
//!   walk and what sizes a library write are numbers the device supplied.
//! - [`Device::write`] sizes a library's cleaning pass from that partition's
//!   [`AllocationUnit`] and the body it is about to send.
//!
//! Nothing here touches the wire: every frame is emitted by [`op`] or [`Session`].

use crate::envelope;
use crate::error::{Error, Result};
use crate::op;
use crate::session::{ReadOnly, ReadWrite, Session};
use crate::transport::Transport;
use crate::wire::{AllocationUnit, Bank, Location, ObjectClass, Partition};

/// A product this crate knows by its USB product id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Product {
    Electro5,
    /// On the bus with Clavia's vendor id, but not a product id this crate has met.
    Unknown(u16),
}

impl Product {
    pub fn from_product_id(id: u16) -> Self {
        match id {
            crate::transport::PRODUCT_ID_ELECTRO5 => Product::Electro5,
            other => Product::Unknown(other),
        }
    }
}

/// What the instrument says it holds: every partition, and each one's banks.
///
/// The partition index is the object class code, so this answers "does this instrument
/// have that class, how many banks does it have, and how large are they" without any
/// constant. The two `(Native)` partitions are carried and never consulted — they are a
/// second view of a library this crate addresses through its user partition.
pub struct Geometry {
    partitions: Vec<Partition>,
    /// One entry per partition, in the same order.
    banks: Vec<BankList>,
}

impl Geometry {
    /// Read both tables: `PARTITIONS`, then `BANKS` for each partition's index in table
    /// order.
    ///
    /// A device that refuses `BANKS` for one partition still has geometry for the rest —
    /// the refusal leaves the session in step — so it is recorded and re-reported by
    /// [`Self::banks`] for that class alone. A transport failure fails the whole read.
    pub async fn read<T: Transport, C>(session: &mut Session<'_, T, C>) -> Result<Self> {
        let partitions = op::partitions(session).await?;
        let mut banks = Vec::with_capacity(partitions.len());
        for partition in &partitions {
            banks.push(match op::banks(session, partition.index).await {
                Ok(list) => Ok(list),
                Err(Error::DeviceStatus(status)) => Err(status),
                Err(e) => return Err(e),
            });
        }
        Ok(Self { partitions, banks })
    }

    pub fn partitions(&self) -> &[Partition] {
        &self.partitions
    }

    /// The partition storing `class`. An instrument without one is an error, never a
    /// default: the whole point of reading the table is not to assume.
    pub fn partition(&self, class: ObjectClass) -> Result<&Partition> {
        Ok(self.entry(class)?.0)
    }

    /// The banks a walk of `class` covers, in table order.
    pub fn banks(&self, class: ObjectClass) -> Result<&[Bank]> {
        match self.entry(class)?.1 {
            Ok(banks) => Ok(banks),
            Err(status) => Err(Error::DeviceStatus(*status)),
        }
    }

    /// The unit `class`'s [`Status`](crate::wire::Status) counters are denominated in.
    pub fn allocation_unit(&self, class: ObjectClass) -> Result<AllocationUnit> {
        self.partition(class)?.allocation_unit()
    }

    fn entry(&self, class: ObjectClass) -> Result<(&Partition, &BankList)> {
        self.partitions
            .iter()
            .zip(&self.banks)
            .find(|(partition, _)| partition.index == class.to_raw())
            .ok_or_else(|| {
                Error::InvalidArgument(format!("the instrument has no {} partition", class.label()))
            })
    }
}

/// One partition's banks, or the status the device refused
/// [`cmd::BANKS`](crate::wire::cmd::BANKS) with.
type BankList = std::result::Result<Vec<Bank>, u32>;

/// An attached instrument. See the module documentation for the shape.
pub struct Device<T: Transport> {
    transport: T,
    product: Product,
    geometry: Option<Geometry>,
}

impl<T: Transport> Device<T> {
    /// Wrap an already-open transport. `product` is whatever identified the thing on the
    /// other end; [`Product::Unknown`] is the honest answer when nothing did.
    pub fn new(transport: T, product: Product) -> Self {
        Self {
            transport,
            product,
            geometry: None,
        }
    }

    pub fn product(&self) -> Product {
        self.product
    }

    /// The transport itself, for what the brackets cannot express — [`op::recover`],
    /// [`Session::probe`], or a backend-specific call.
    pub fn transport(&mut self) -> &mut T {
        &mut self.transport
    }

    pub fn into_transport(self) -> T {
        self.transport
    }

    /// Run a chain of read-only operations in one transaction.
    ///
    /// The transaction is committed whether the chain succeeded or failed, and the
    /// chain's error is the one reported: a close that fails after it usually fails
    /// because of it.
    pub async fn read<R>(
        &mut self,
        class: ObjectClass,
        f: impl AsyncFnOnce(&mut Session<'_, T, ReadOnly>) -> Result<R>,
    ) -> Result<R> {
        let session = Session::open(&mut self.transport, class).await?;
        bracket(session, f).await
    }

    /// Run a chain that may mutate the instrument, in one transaction.
    ///
    /// The name is the consent: this is the only route to a [`ReadWrite`] session, and a
    /// write can destroy an object the caller never named.
    pub async fn destructive<R>(
        &mut self,
        class: ObjectClass,
        f: impl AsyncFnOnce(&mut Session<'_, T, ReadWrite>) -> Result<R>,
    ) -> Result<R> {
        let session = Session::open(&mut self.transport, class)
            .await?
            .allow_destructive_writes();
        bracket(session, f).await
    }

    /// The instrument's [`Geometry`], read on first use and kept.
    ///
    /// The tables are static configuration — storing and deleting content leaves every
    /// field of both unchanged — so one read serves the life of the `Device`.
    ///
    /// Confirmed on hardware.
    pub async fn geometry(&mut self) -> Result<&Geometry> {
        if self.geometry.is_none() {
            // Any class opens a session; both tables are device-wide.
            let read = self
                .read(ObjectClass::Program, async |s| Geometry::read(s).await)
                .await?;
            self.geometry = Some(read);
        }
        Ok(self.geometry.as_ref().expect("just read"))
    }

    /// Write a file into a slot, preparing library space first where the class needs it.
    ///
    /// A library write is refused `0x16` without a prepared block per storage block of
    /// body, so a library class reserves in the same transaction as the transfer, sized
    /// by that partition's [`AllocationUnit`] and the body the file carries — the CBIN
    /// body, which is shorter than the file by its header.
    ///
    /// ⚠️ Most classes refuse a write into an occupied slot with status `0x4`; see
    /// [`ObjectClass::overwrites_in_place`]. Emptying the slot first, and putting the
    /// occupant back when the write fails, is the caller's to sequence.
    pub async fn write(
        &mut self,
        class: ObjectClass,
        at: Location,
        file: &[u8],
        name: &str,
        timestamp: u32,
    ) -> Result<()> {
        if !class.is_library() {
            return self
                .destructive(class, async |s| {
                    op::write(s, at, file, name, timestamp).await
                })
                .await;
        }
        let body_len = envelope::unwrap(file)?.body.0.len();
        let blocks = self
            .geometry()
            .await?
            .allocation_unit(class)?
            .blocks_for(body_len)?;
        self.destructive(class, async |s| {
            op::reserve(s, blocks).await?;
            op::write(s, at, file, name, timestamp).await
        })
        .await
    }
}

#[cfg(feature = "nusb")]
impl Device<crate::transport::UsbTransport> {
    /// Open the first attached Clavia device, naming it from its USB product id.
    pub fn detect() -> Result<Self> {
        let info = crate::transport::usb::list()?
            .into_iter()
            .next()
            .ok_or_else(|| Error::Transport("no Clavia device found".into()))?;
        let product = Product::from_product_id(info.product_id());
        Ok(Self::new(
            crate::transport::UsbTransport::open(&info)?,
            product,
        ))
    }
}

/// Run `f` and close the session on both paths, keeping `f`'s error over the close's.
async fn bracket<T: Transport, C, R>(
    mut session: Session<'_, T, C>,
    f: impl AsyncFnOnce(&mut Session<'_, T, C>) -> Result<R>,
) -> Result<R> {
    let r = f(&mut session).await;
    // A chain that bailed on a desync released the session already, and `commit` on a
    // released session sends nothing and reports `Ok`.
    let closed = session.commit().await;
    match r {
        Ok(v) => closed.map(|()| v),
        Err(e) => Err(e),
    }
}
