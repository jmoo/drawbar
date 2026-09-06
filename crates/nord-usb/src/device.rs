//! An instrument as a value: a transport and the session bracket every operation runs
//! inside.
//!
//! [`op`] is the vocabulary — one capture-pinned function per protocol operation — and
//! [`Session`] the transaction they run in. This is where they compose:
//!
//! - [`Device::read`] and [`Device::destructive`] open a transaction, hand the chain the
//!   raw [`Session`], and attempt cleanup before returning.
//! - [`Geometry`] is the instrument's own partition and bank tables, so what bounds a
//!   walk and what sizes a library write are numbers the device supplied.
//! - [`Device::write`] sizes a library's cleaning pass from that partition's
//!   [`AllocationUnit`] and the body it is about to send.
//! - [`Device::take_changed`] carries the instrument's own "I changed" notification out
//!   of the transaction it arrived in.
//!
//! Nothing here touches the wire: every frame is emitted by [`op`] or [`Session`].

use crate::error::{Error, Result};
use crate::op;
use crate::session::{ReadOnly, ReadWrite, Session};
use crate::transport::Transport;
use crate::wire::{AllocationUnit, Bank, Location, ObjectClass, Partition};

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

    /// Every partition in table order with its banks, or the status the device refused
    /// [`cmd::BANKS`](crate::wire::cmd::BANKS) with for that one. This is the whole
    /// table, `(Native)` partitions included, rather than the classes this crate names.
    pub fn entries(&self) -> impl Iterator<Item = (&Partition, std::result::Result<&[Bank], u32>)> {
        self.partitions
            .iter()
            .zip(&self.banks)
            .map(|(partition, banks)| (partition, banks.as_deref().map_err(|&status| status)))
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
    geometry: Option<Geometry>,
    changed: bool,
}

impl<T: Transport> Device<T> {
    /// Wrap an already-open transport.
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            geometry: None,
            changed: false,
        }
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
    /// Cleanup is attempted whether the chain succeeds or fails. When both fail, the
    /// chain's error is reported.
    ///
    /// ⚠️ The close is what clears the instrument's progress label. A transaction
    /// abandoned after a read has painted `"Uploading..."` leaves that label on the
    /// display with no way out but a power cycle; the bracket exists so no `?` can do
    /// that.
    pub async fn read<R>(
        &mut self,
        class: ObjectClass,
        f: impl AsyncFnOnce(&mut Session<'_, T, ReadOnly>) -> Result<R>,
    ) -> Result<R> {
        let session = Session::open(&mut self.transport, class).await?;
        bracket(&mut self.changed, session, f).await
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
        bracket(&mut self.changed, session, f).await
    }

    /// Whether the instrument reported changing under us since this was last asked, and
    /// clear it.
    ///
    /// Every bracket preserves its session's [`Session::instrument_changed`] flag, so
    /// state read during any completed transaction may be stale.
    pub fn take_changed(&mut self) -> bool {
        std::mem::take(&mut self.changed)
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
        if class.is_library() {
            let unit = self.geometry().await?.allocation_unit(class)?;
            return self
                .destructive(class, async |s| {
                    op::write_library(s, unit, at, file, name, timestamp).await
                })
                .await;
        }
        self.destructive(class, async |s| op::write(s, at, file, name, timestamp).await)
            .await
    }
}

/// Run `f` and attempt cleanup on both paths, keeping `f`'s error over cleanup's.
///
/// `changed` collects the session's [`Session::instrument_changed`] whichever way the
/// chain went: a notification that arrived is a fact about the instrument, not about the
/// operation that happened to be running.
async fn bracket<T: Transport, C, R>(
    changed: &mut bool,
    mut session: Session<'_, T, C>,
    f: impl AsyncFnOnce(&mut Session<'_, T, C>) -> Result<R>,
) -> Result<R> {
    let result = f(&mut session).await;
    let (closed, session_changed) = session.commit_observing_changed().await;
    *changed |= session_changed;
    match result {
        Ok(v) => closed.map(|()| v),
        Err(e) => Err(e),
    }
}
