//! An instrument as a value: transport + per-product profile + session brackets.
//!
//! [`op`] gives the vocabulary — one capture-pinned function per protocol operation —
//! and [`Session`] the transaction every operation must run inside. What neither gives
//! is the sentence: "replace 2:14, backing the occupant up first, and if the panel is
//! sitting on 2:14 make it pick up the new content" is several operations, one
//! transaction, a recovery policy, and a close that must run on every path. [`Device`]
//! is where sentences live:
//!
//! - [`Device::read`] / [`Device::destructive`] — the session bracket. Opens a
//!   transaction, runs an async closure chaining any number of operations over a
//!   [`Txn`], and commits on success *and* failure, keeping the operation's error over
//!   the close's. The discipline every caller owes ([`Session::commit`]) becomes the
//!   only path.
//! - [`Device::replace_program`], [`Device::update_focused`], [`Device::inventory`] —
//!   named intents composed from the primitives, with the recovery policy written once.
//! - [`Profile`] — what is known about the product itself, resolved from the USB
//!   product id. Advisory throughout: a wrong profile degrades to the device refusing
//!   at runtime, never to this crate refusing an operation the instrument would accept.
//!
//! Nothing here touches the wire directly: every frame is emitted by [`op`] or
//! [`Session`], so the capture-verified byte shapes are composed, not re-implemented.

use crate::error::{Error, Result};
use crate::op;
use crate::session::{ReadOnly, ReadWrite, Session};
use crate::transport::Transport;
use crate::wire::{Bank, Dependency, Location, ObjectClass, Partition, ProgramInfo, Status};

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

    /// What is known about this product's protocol behavior.
    pub fn profile(self) -> Profile {
        match self {
            Product::Electro5 => Profile {
                product: self,
                inventory: &[
                    ObjectClass::Piano,
                    ObjectClass::Sample,
                    ObjectClass::Program,
                    ObjectClass::SetList,
                ],
                overwrite_in_place: false,
                enumeration_disabled_after_write: true,
            },
            // The conservative reading for an unmet product: assume the Electro 5's
            // restrictions, and sweep only the classes every observed instrument has.
            Product::Unknown(_) => Profile {
                product: self,
                inventory: &[ObjectClass::Program],
                overwrite_in_place: false,
                enumeration_disabled_after_write: true,
            },
        }
    }
}

/// Per-product protocol knowledge. Data consulted by the intents, never a gate: the
/// device's own refusal is always the authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Profile {
    pub product: Product,
    /// Classes worth querying in an inventory sweep.
    pub inventory: &'static [ObjectClass],
    /// Whether a write into an occupied slot succeeds. `false` everywhere observed:
    /// the device refuses with status `0x4`, so replacing is delete-then-write
    /// ([`Device::replace_program`]). Confirmed on hardware for the Electro 5.
    pub overwrite_in_place: bool,
    /// Whether [`op::next_occupied`] stops answering after the first write each power
    /// cycle ([`op::ENUMERATION_DISABLED`]). Confirmed on hardware for the Electro 5.
    pub enumeration_disabled_after_write: bool,
}

/// An attached instrument: the transport it is reached through, plus what is known
/// about the product. See the module doc for the shape.
pub struct Device<T: Transport> {
    transport: T,
    profile: Profile,
}

/// One open transaction, with a method per operation so a chain inside a bracket reads
/// as calls: `t.info(at).await?`, `t.select(at).await?`. Thin delegation to [`op`] —
/// no logic of its own, and the raw session stays reachable for anything not wrapped.
pub struct Txn<'t, T: Transport, C> {
    session: Session<'t, T, C>,
}

impl<'t, T: Transport, C> Txn<'t, T, C> {
    /// The wrapped session, for operations [`Txn`] has no method for.
    pub fn session(&mut self) -> &mut Session<'t, T, C> {
        &mut self.session
    }

    /// See [`Session::instrument_changed`].
    pub fn instrument_changed(&self) -> bool {
        self.session.instrument_changed()
    }

    pub async fn status(&mut self) -> Result<Status> {
        op::status(&mut self.session).await
    }

    pub async fn info(&mut self, at: Location) -> Result<ProgramInfo> {
        op::info(&mut self.session, at).await
    }

    pub async fn read_program(&mut self, at: Location) -> Result<Vec<u8>> {
        op::read_program(&mut self.session, at).await
    }

    pub async fn read_body(&mut self, at: Location) -> Result<Vec<u8>> {
        op::read_body(&mut self.session, at).await
    }

    pub async fn select(&mut self, at: Location) -> Result<()> {
        op::select(&mut self.session, at).await
    }

    pub async fn focus(&mut self) -> Result<Location> {
        op::focus(&mut self.session).await
    }

    pub async fn partitions(&mut self) -> Result<Vec<Partition>> {
        op::partitions(&mut self.session).await
    }

    pub async fn banks(&mut self, partition: u32) -> Result<Vec<Bank>> {
        op::banks(&mut self.session, partition).await
    }

    pub async fn check_address(&mut self, at: Location) -> Result<Option<String>> {
        op::check_address(&mut self.session, at).await
    }

    pub async fn occupied_slots(&mut self, cap: usize) -> Result<Vec<Location>> {
        op::occupied_slots(&mut self.session, cap).await
    }

    pub async fn next_occupied(&mut self, at: Location) -> Result<Option<Location>> {
        op::next_occupied(&mut self.session, at).await
    }

    pub async fn dependencies(&mut self, at: Location) -> Result<Vec<Dependency>> {
        op::dependencies(&mut self.session, at).await
    }

    pub async fn required_dependencies(&mut self, at: Location) -> Result<Vec<Dependency>> {
        op::required_dependencies(&mut self.session, at).await
    }
}

impl<T: Transport> Txn<'_, T, ReadWrite> {
    pub async fn write_program(&mut self, at: Location, file: &[u8], timestamp: u32) -> Result<()> {
        op::write_program(&mut self.session, at, file, timestamp).await
    }

    pub async fn delete(&mut self, at: Location) -> Result<()> {
        op::delete(&mut self.session, at).await
    }

    pub async fn rename(&mut self, at: Location, name: &str) -> Result<()> {
        op::rename(&mut self.session, at, name).await
    }

    pub async fn move_object(&mut self, from: Location, to: Location) -> Result<()> {
        op::move_object(&mut self.session, from, to).await
    }

    pub async fn duplicate(&mut self, from: Location, to: Location) -> Result<()> {
        op::duplicate(&mut self.session, from, to).await
    }
}

/// What [`Device::replace_program`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replaced {
    /// What the slot held before, when it held anything. Its bytes were read back and
    /// held in memory until the write succeeded.
    pub previous: Option<ProgramInfo>,
    /// The panel had the slot focused, so it was re-selected after the write — without
    /// that the panel keeps playing the old content. Confirmed on hardware.
    pub refocused: bool,
}

/// What happened to a slot's previous contents when [`Device::replace_program`] failed.
#[derive(Debug)]
pub enum Occupant {
    /// The failure came before anything was deleted; the slot is as it was.
    Untouched,
    /// The write failed after the delete, and the backup was written back. The slot
    /// holds its original contents again.
    Restored,
    /// The write failed after the delete and the restore failed too. `backup` is the
    /// only remaining copy of the slot's contents — the caller must not drop it
    /// silently.
    Lost {
        backup: Vec<u8>,
        restore_error: Error,
    },
}

/// A failed [`Device::replace_program`], reporting both the failing operation's error
/// and the fate of the slot's previous contents.
#[derive(Debug)]
pub struct ReplaceError {
    pub error: Error,
    pub occupant: Occupant,
}

impl std::fmt::Display for ReplaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.occupant {
            Occupant::Untouched => write!(f, "{} (the slot was left untouched)", self.error),
            Occupant::Restored => write!(
                f,
                "{} (the slot's previous contents were restored)",
                self.error
            ),
            Occupant::Lost {
                backup,
                restore_error,
            } => write!(
                f,
                "{} (restoring failed too: {restore_error}; the slot is empty and the \
                 only copy of its {} former bytes is in memory)",
                self.error,
                backup.len()
            ),
        }
    }
}

impl std::error::Error for ReplaceError {}

impl<T: Transport> Device<T> {
    /// Wrap an already-open transport. `profile` says what the caller knows about the
    /// product on the other end; [`Product::Unknown`]`(0).profile()` is the honest
    /// answer when nothing identified it.
    pub fn new(transport: T, profile: Profile) -> Self {
        Self { transport, profile }
    }

    pub fn profile(&self) -> &Profile {
        &self.profile
    }

    /// The transport itself, for what the brackets cannot express — [`op::recover`],
    /// probing, or backend-specific calls like endpoint-0 identity.
    pub fn transport(&mut self) -> &mut T {
        &mut self.transport
    }

    pub fn into_transport(self) -> T {
        self.transport
    }

    /// Run a chain of read-only operations in one transaction.
    ///
    /// The transaction is committed whether the chain succeeds or fails; when both the
    /// chain and the close fail, the chain's error is reported — the close's failure is
    /// usually its consequence.
    pub async fn read<R>(
        &mut self,
        class: ObjectClass,
        f: impl AsyncFnOnce(&mut Txn<'_, T, ReadOnly>) -> Result<R>,
    ) -> Result<R> {
        let session = Session::open(&mut self.transport, class).await?;
        run(Txn { session }, f).await
    }

    /// Run a chain that may mutate the device, in one transaction.
    ///
    /// The name is the consent: this is the only path to a [`ReadWrite`] session, and
    /// device writes can destroy patches — callers should back up first (or use
    /// [`Device::replace_program`], which does).
    pub async fn destructive<R>(
        &mut self,
        class: ObjectClass,
        f: impl AsyncFnOnce(&mut Txn<'_, T, ReadWrite>) -> Result<R>,
    ) -> Result<R> {
        let session = Session::open(&mut self.transport, class)
            .await?
            .allow_destructive_writes();
        run(Txn { session }, f).await
    }

    /// Query every class the profile lists, one transaction each. A class that errors
    /// is skipped rather than failing the sweep — instruments differ in which classes
    /// they answer for, and the profile is advisory.
    pub async fn inventory(&mut self) -> Result<Vec<Status>> {
        let mut out = Vec::new();
        for &class in self.profile.inventory {
            match self.read(class, async |t| t.status().await).await {
                Ok(s) => out.push(s),
                Err(_) => continue,
            }
        }
        Ok(out)
    }

    /// Put `file` into a slot, replacing whatever it holds, in one transaction:
    ///
    /// 1. the panel's focus is read, so a write into the focused slot can end with a
    ///    re-select — without it the panel keeps playing the old content (confirmed on
    ///    hardware);
    /// 2. an occupant is read back in full before being deleted — the device refuses
    ///    to overwrite in place (status `0x4`), so the slot is genuinely empty between
    ///    delete and write, and the backup is the only copy;
    /// 3. the file is written, and the panel re-pointed when step 1 said so.
    ///
    /// On failure after the delete, a **recovery transaction** writes the backup
    /// back — a separate transaction, because the failing operation may have released
    /// the first. The [`ReplaceError`] says which of the three fates the occupant met;
    /// [`Occupant::Lost`] carries the backup bytes, which the caller must land
    /// somewhere durable rather than drop.
    ///
    /// `class` must be a slot-addressed class ([`ObjectClass::Program`] or
    /// [`ObjectClass::SetList`] — the classes [`op::write_program`] is verified for).
    pub async fn replace_program(
        &mut self,
        class: ObjectClass,
        at: Location,
        file: &[u8],
        timestamp: u32,
    ) -> std::result::Result<Replaced, ReplaceError> {
        let overwrite_in_place = self.profile.overwrite_in_place;
        let mut previous: Option<ProgramInfo> = None;
        let mut backup: Option<Vec<u8>> = None;
        let mut deleted = false;
        let mut refocused = false;

        let chain = self
            .destructive(class, async |t| {
                // A focus the class does not keep (status 0x15 from the library
                // classes, 0x1 when nothing is loaded) is a refusal, not a fault: the
                // session stays in step and the write simply won't refocus.
                let focus = match t.focus().await {
                    Ok(f) => Some(f),
                    Err(Error::DeviceStatus(_)) => None,
                    Err(e) => return Err(e),
                };

                match t.info(at).await {
                    Ok(info) => previous = Some(info),
                    // Status 1: the slot is vacant, which is the simple case.
                    Err(Error::DeviceStatus(1)) => {}
                    Err(e) => return Err(e),
                }

                if previous.is_some() {
                    // Nothing is deleted until the backup is in hand.
                    backup = Some(t.read_program(at).await?);
                    if !overwrite_in_place {
                        t.delete(at).await?;
                        deleted = true;
                    }
                }

                t.write_program(at, file, timestamp).await?;

                if focus == Some(at) {
                    t.select(at).await?;
                    refocused = true;
                }
                Ok(())
            })
            .await;

        match chain {
            Ok(()) => Ok(Replaced {
                previous,
                refocused,
            }),
            Err(error) => {
                let occupant = match backup.filter(|_| deleted) {
                    None => Occupant::Untouched,
                    Some(bytes) => {
                        let restored = self
                            .destructive(class, async |t| {
                                t.write_program(at, &bytes, timestamp).await
                            })
                            .await;
                        match restored {
                            Ok(()) => Occupant::Restored,
                            Err(restore_error) => Occupant::Lost {
                                backup: bytes,
                                restore_error,
                            },
                        }
                    }
                };
                Err(ReplaceError { error, occupant })
            }
        }
    }

    /// Replace whatever the panel currently has loaded with `file` — resolve the
    /// focus, then [`Device::replace_program`] at it, which ends by re-selecting so
    /// the panel picks the new content up. Returns where it landed.
    pub async fn update_focused(
        &mut self,
        class: ObjectClass,
        file: &[u8],
        timestamp: u32,
    ) -> std::result::Result<(Location, Replaced), ReplaceError> {
        let at = self
            .read(class, async |t| t.focus().await)
            .await
            .map_err(|error| ReplaceError {
                error,
                occupant: Occupant::Untouched,
            })?;
        let replaced = self.replace_program(class, at, file, timestamp).await?;
        Ok((at, replaced))
    }
}

/// The bracket's second half, shared by both capabilities: run the chain, commit on
/// every path, keep the chain's error over the close's.
async fn run<T: Transport, C, R>(
    mut txn: Txn<'_, T, C>,
    f: impl AsyncFnOnce(&mut Txn<'_, T, C>) -> Result<R>,
) -> Result<R> {
    let r = f(&mut txn).await;
    // A chain that bailed on a desync has already been released; `commit` on a
    // released session is a no-op, so this is safe on every path.
    let closed = txn.session.commit().await;
    match r {
        Ok(v) => closed.map(|()| v),
        Err(e) => Err(e),
    }
}

#[cfg(feature = "nusb")]
impl Device<crate::transport::UsbTransport> {
    /// Open the first Clavia instrument on the bus and resolve its profile from the
    /// USB product id. Endpoint-0 identity stays reachable through
    /// [`Device::transport`].
    pub fn detect() -> Result<Self> {
        let info = crate::transport::usb::list()?
            .into_iter()
            .next()
            .ok_or_else(|| Error::Transport("no Clavia device found".into()))?;
        let product = Product::from_product_id(info.product_id());
        let transport = crate::transport::UsbTransport::open(&info)?;
        Ok(Self::new(transport, product.profile()))
    }
}
