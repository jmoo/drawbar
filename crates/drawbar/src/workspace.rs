//! This computer: the assets held in memory, and the ways bytes get in and out of them.
//!
//! An entity is bytes first and a decode second. A file that does not parse still gets a
//! row, with its error shown and its raw body exportable, because reporting a bad file is
//! the point of opening it.
//!
//! [`crate::browser`] draws the list; this module holds the model and the file dialogs.

use std::borrow::Cow;
use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;

use eframe::egui;
use nord_format::accept::{self, Slot};
use nord_format::cbin::{Cbin, Generation, Header};
use nord_format::formats::{ne5, ns2, ns3, ns4, nsmpproj};
use nord_format::{Entity, OrganPreset, PianoPreset, Program, Synth};
use nord_usb::{Location, ObjectClass};

use crate::log::Log;
use crate::newproject::{Draft, Making};
use crate::ondisk::{self, OnDisk};
use crate::queue::Queue;
use crate::rewrite::{Edit, Rewrite};
use crate::store::{names, CopyOf, LibPath, Outside};
use crate::summary::{Naming, Plays, Summary, Verdict};
use crate::work;

/// Where an entity came from.
#[derive(Clone)]
pub enum Origin {
    File(String),
    Device {
        class: ObjectClass,
        at: Location,
    },
    Fresh,
    /// The occupant of a slot that a failed write could not put back.
    Rescued {
        at: Location,
    },
}

impl Origin {
    pub fn label(&self) -> String {
        match self {
            Origin::File(name) => format!("Opened from {name}"),
            Origin::Device { class, at } => {
                format!("Copied from {}", crate::strings::place(*class, *at))
            }
            Origin::Fresh => "New, not saved anywhere yet".into(),
            Origin::Rescued { at } => format!("Rescued from {}", crate::strings::shown(*at)),
        }
    }

    /// The slot this came off, for the tab header's link back to it.
    pub fn slot(&self) -> Option<(ObjectClass, Location)> {
        match self {
            Origin::Device { class, at } => Some((*class, *at)),
            _ => None,
        }
    }
}

/// Whether re-encoding a decode reproduced the bytes it came from.
#[derive(Clone)]
pub enum VerifyState {
    Ok,
    /// A file left on disk whose stored checksum matches its body. A file that large is
    /// checked by its checksum, never re-encoded.
    Checked,
    /// A file left on disk whose checksum is still being checked.
    Checking,
    /// Offset of the first byte that came back different.
    Differs {
        at: usize,
    },
    /// Re-encoding refused.
    Failed(String),
    /// Nothing to check, and why.
    NotApplicable(&'static str),
    /// A file from the library not decoded yet, or not read yet. It is read once
    /// something needs it, and decoded off the frame, or at once by
    /// [`Workspace::read_now`] for whatever acts on it.
    Reading,
    /// A file from the library that could not be read, and why.
    NotRead(String),
    /// A file from the library not read this session, whose
    /// [`Summary`] says what a read of it found before. It is
    /// read once something needs its bytes.
    Remembered(Verdict),
}

impl VerifyState {
    pub fn badge(&self) -> &'static str {
        match self {
            VerifyState::Ok | VerifyState::Checked => "ok",
            VerifyState::Checking => "checking…",
            VerifyState::Reading => "reading…",
            VerifyState::Differs { .. } => "differs",
            VerifyState::Failed(_) => "failed",
            VerifyState::NotRead(_) => "not read",
            VerifyState::NotApplicable(_) => "n/a",
            VerifyState::Remembered(verdict) => match verdict {
                Verdict::Ok | Verdict::Checked => "ok",
                Verdict::Differs { .. } => "differs",
                Verdict::Failed => "failed",
                Verdict::NotApplicable => "n/a",
            },
        }
    }

    pub fn detail(&self) -> String {
        match self {
            VerifyState::Ok => "re-encoded byte-for-byte".into(),
            VerifyState::Checked => "its stored checksum matches its body".into(),
            VerifyState::Checking => "its checksum is being checked".into(),
            VerifyState::Reading => "it is still being read".into(),
            VerifyState::Differs { at } => format!("first difference at byte {at:#06x}"),
            VerifyState::Failed(why) | VerifyState::NotRead(why) => why.clone(),
            VerifyState::NotApplicable(why) => (*why).to_string(),
            VerifyState::Remembered(Verdict::Differs { at }) => {
                format!("first difference at byte {at:#06x} when it was last read")
            }
            VerifyState::Remembered(_) => "as it was when it was last read".into(),
        }
    }

    pub fn color(&self, visuals: &egui::Visuals) -> egui::Color32 {
        match self {
            VerifyState::Ok
            | VerifyState::Checked
            | VerifyState::Remembered(Verdict::Ok | Verdict::Checked) => crate::app::good(visuals),
            VerifyState::Differs { .. }
            | VerifyState::Failed(_)
            | VerifyState::NotRead(_)
            | VerifyState::Remembered(Verdict::Differs { .. } | Verdict::Failed) => {
                crate::app::bad(visuals)
            }
            VerifyState::Checking
            | VerifyState::Reading
            | VerifyState::NotApplicable(_)
            | VerifyState::Remembered(Verdict::NotApplicable) => visuals.weak_text_color(),
        }
    }

    /// What a row says about it, where it says anything.
    pub fn note(&self) -> Option<&'static str> {
        match self {
            VerifyState::Checking => Some("checking…"),
            VerifyState::Reading => Some("reading…"),
            VerifyState::Failed(_) | VerifyState::Remembered(Verdict::Failed) => {
                Some("failed verification")
            }
            VerifyState::NotRead(_) => Some("not read"),
            VerifyState::Ok
            | VerifyState::Checked
            | VerifyState::Differs { .. }
            | VerifyState::NotApplicable(_)
            | VerifyState::Remembered(_) => None,
        }
    }
}

/// The container facts, read once at ingest.
///
/// ⚠️ Reading them streams the whole file to check the checksum and hash its body, so
/// it happens at ingest and never per frame. A piano library is hundreds of megabytes.
#[derive(Clone)]
pub struct Container {
    pub header: Header,
    /// Where the body sits in the file, checked against the file's length when it was
    /// read. Anything that needs the body reads this range instead of adding a declared
    /// length to its own start offset.
    pub body: std::ops::Range<usize>,
    pub checksum_ok: bool,
    /// `crc32:` or `crc16:`. The two generations store different checksums in different
    /// places.
    pub checksum_label: &'static str,
    pub checksum: String,
    /// The checksum a slot holding this body reports. See
    /// [`nord_format::cbin::Info::body_crc32`].
    pub body_crc32: u32,
}

impl Container {
    fn read(bytes: &[u8]) -> Option<Container> {
        let info = nord_format::cbin::inspect(&mut std::io::Cursor::new(bytes)).ok()?;
        let body = body_of(&info)?;
        Some(Container::of(info, body))
    }

    /// The facts of a file left on disk, from what a checksum pass over it found. A pass
    /// that read another header than the index did read another file.
    fn of_file(file: &OnDisk, sums: ondisk::Sums) -> Result<Container, String> {
        if sums.info.header != *file.index.header() {
            return Err("the file changed while it was read".to_string());
        }
        let body = body_of(&sums.info).ok_or("the body is larger than this machine can address")?;
        Ok(Container::of(sums.info, body))
    }

    fn of(info: nord_format::cbin::Info, body: std::ops::Range<usize>) -> Container {
        let (checksum_label, checksum) = match info.header.generation {
            Generation::V0 => ("crc16:", format!("{:#06x}", info.stored_checksum)),
            Generation::V1 => ("crc32:", format!("{:#010x}", info.stored_checksum)),
        };
        Container {
            header: info.header,
            body,
            checksum_ok: info.checksum_ok,
            checksum_label,
            checksum,
            body_crc32: info.body_crc32,
        }
    }

    pub fn tag(&self) -> String {
        String::from_utf8_lossy(&self.header.tag).into_owned()
    }

    /// The body's length in bytes.
    pub fn body_len(&self) -> u64 {
        self.body.len() as u64
    }
}

/// Where the body sits in a file `inspect` read.
fn body_of(info: &nord_format::cbin::Info) -> Option<std::ops::Range<usize>> {
    let start = usize::try_from(info.header.generation.body_start()).ok()?;
    let end = start.checked_add(usize::try_from(info.body_len).ok()?)?;
    Some(start..end)
}

/// An asset's bytes, which its saved baseline shares while they are the same bytes. An
/// edit puts new bytes in place; nothing writes through a shared allocation.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Bytes(Arc<[u8]>);

impl Bytes {
    /// Whether these and `other` are one allocation, held once.
    pub fn shares(&self, other: &Bytes) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl std::ops::Deref for Bytes {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        &self.0
    }
}

impl AsRef<[u8]> for Bytes {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl From<Vec<u8>> for Bytes {
    fn from(bytes: Vec<u8>) -> Bytes {
        Bytes(bytes.into())
    }
}

impl PartialEq<[u8]> for Bytes {
    fn eq(&self, other: &[u8]) -> bool {
        *self.0 == *other
    }
}

impl PartialEq<&[u8]> for Bytes {
    fn eq(&self, other: &&[u8]) -> bool {
        *self.0 == **other
    }
}

impl PartialEq<Vec<u8>> for Bytes {
    fn eq(&self, other: &Vec<u8>) -> bool {
        *self.0 == **other
    }
}

impl std::fmt::Debug for Bytes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// What an asset was last saved as: the bytes, and the checksum a slot holding them
/// would report.
///
/// ⚠️ The checksum is computed when the baseline moves and never per frame. Computing one
/// streams the whole body, and every listed row asks for it while the library is shown.
#[derive(Clone, Default)]
pub struct Baseline {
    pub bytes: Bytes,
    /// The checksum a slot holding these bytes would report. [`crate::device::link`] and
    /// [`crate::library::agrees`] both decide on it.
    ///
    /// `None` for bytes that are not a CBIN container. See [`Container::body_crc32`].
    pub crc32: Option<u32>,
    /// The [`LocalEntity::stamp`] of these bytes: the asset's current stamp while it
    /// still holds them, and a separate stamp once it does not.
    ///
    /// ⚠️ [`LocalEntity::is_unsaved`] compares stamps, not bodies. Comparing two bodies
    /// costs time proportional to the library's size, and the header alone asks twice a
    /// frame.
    pub stamp: u64,
    /// The file that holds these bytes, read by range and never held: `bytes` is then
    /// empty, and `crc32` is `None` until the file's checksum has been checked.
    pub file: Option<Arc<OnDisk>>,
    /// CRC-32 over all of `bytes`, where it was taken as they arrived. See
    /// [`Baseline::whole_crc`].
    pub(crate) bytes_crc: Option<u32>,
    /// The length of the file that holds these bytes, while nothing has read it: `bytes`
    /// is then empty, and `file` is `None`.
    pub unread: Option<u64>,
}

impl Baseline {
    /// The baseline of bytes not yet inspected, stamped with `stamp`.
    pub(crate) fn read(bytes: Bytes, stamp: u64) -> Baseline {
        let crc32 = Container::read(&bytes).map(|held| held.body_crc32);
        Baseline {
            bytes_crc: Some(nord_format::crc::crc32(&bytes)),
            bytes,
            crc32,
            stamp,
            file: None,
            unread: None,
        }
    }

    fn on_disk(file: Arc<OnDisk>, stamp: u64) -> Baseline {
        Baseline {
            bytes: Bytes::default(),
            crc32: None,
            stamp,
            file: Some(file),
            bytes_crc: None,
            unread: None,
        }
    }

    /// CRC-32 over the whole of these bytes, where something has taken it: the file's,
    /// for a baseline resting in it, once its check has read it.
    pub fn whole_crc(&self) -> Option<u32> {
        match &self.file {
            Some(file) => file.known_crc(),
            None => self.bytes_crc,
        }
    }

    /// How many bytes these are.
    pub fn size(&self) -> u64 {
        match (&self.file, self.unread) {
            (Some(file), _) => file.len,
            (None, Some(len)) => len,
            (None, None) => self.bytes.len() as u64,
        }
    }

    /// Whether a slot holding `bytes` reports what a slot holding these does: the same
    /// length and the same checksum. Nothing is read.
    fn reports(&self, bytes: &[u8]) -> bool {
        self.size() == bytes.len() as u64
            && self.crc32.is_some()
            && self.crc32 == Container::read(bytes).map(|held| held.body_crc32)
    }

    /// Whether these are `bytes`. Never, for bytes not read yet.
    fn holds(&self, bytes: &[u8]) -> bool {
        match (&self.file, self.unread) {
            (Some(file), _) => file.holds(bytes),
            (None, Some(_)) => false,
            (None, None) => self.bytes == bytes,
        }
    }
}

/// A write this app made: the slot it wrote to, and the checksum of the bytes it wrote.
///
/// This is the only thing this app knows about a slot without reading it back. It is
/// evidence for a class whose slots report no checksum, and only while the asset is
/// still saved as those bytes: `crc32` is compared with [`Baseline::crc32`], which
/// changes as soon as the asset is saved as anything else.
#[derive(Clone, Copy)]
pub struct Wrote {
    pub class: ObjectClass,
    pub at: Location,
    pub crc32: u32,
}

/// What a set of bytes decodes to, worked out once, when they land.
struct Decoded {
    container: Option<Container>,
    entity: Option<Box<Entity>>,
    plays: Option<Plays>,
    parse_error: Option<String>,
    verify: VerifyState,
    is_text: bool,
    /// CRC-32 over all the bytes, where they were read through.
    crc: Option<u32>,
}

impl Decoded {
    /// What a decode that never answered leaves.
    fn failed(why: &str) -> Decoded {
        Decoded {
            container: None,
            entity: None,
            plays: None,
            parse_error: Some(why.to_string()),
            verify: VerifyState::Failed(why.to_string()),
            is_text: false,
            crc: None,
        }
    }

    fn of(bytes: &[u8]) -> Decoded {
        let (entity, parse_error) = match nord_format::from_stream(&mut std::io::Cursor::new(bytes))
        {
            Ok(entity) => (Some(Box::new(entity)), None),
            Err(e) => (None, Some(e.to_string())),
        };
        let verify = match &entity {
            Some(entity) => verify(entity, bytes),
            None => VerifyState::NotApplicable("the file did not decode"),
        };
        Decoded {
            container: Container::read(bytes),
            plays: entity.as_deref().and_then(Plays::of),
            entity,
            parse_error,
            verify,
            is_text: crate::document::text::is_text(bytes),
            crc: Some(nord_format::crc::crc32(bytes)),
        }
    }
}

/// One object held in memory: its bytes, what they decode to, and how they got here.
pub struct LocalEntity {
    /// Stable across reordering, so a selection survives a removal.
    pub id: u64,
    pub name: String,
    /// Where its file is in the library. `None` for a view of a slot, and for a kept
    /// asset whose file has not been placed yet.
    ///
    /// ⚠️ While it is set, [`LocalEntity::name`] is its last component. Set both through
    /// [`Workspace::place`].
    pub path: Option<LibPath>,
    pub origin: Origin,
    /// ⚠️ Empty while the asset [`rests`](LocalEntity::rests) in its file. Anything that
    /// needs the whole body asks [`LocalEntity::whole`], and its length is
    /// [`LocalEntity::size`].
    pub bytes: Bytes,
    /// Boxed, since a decode is kilobytes and most assets of a large library have none.
    pub entity: Option<Box<Entity>>,
    /// The library its decode plays, taken with the decode.
    ///
    /// ⚠️ Taken off the frame: finding it lists every field of the body, and every listed
    /// row asks for it.
    pub plays: Option<Plays>,
    pub parse_error: Option<String>,
    pub container: Option<Container>,
    /// Whether the bytes are a note, from `document::text::is_text`.
    ///
    /// ⚠️ Computed when the bytes land and never per frame: deciding it walks every
    /// byte, and every listed row asks for its kind on every frame.
    pub is_text: bool,
    pub verify: VerifyState,
    /// What this asset was last saved as. The asset is unsaved when its bytes differ
    /// from these or an editor holds an edit not yet applied to them. See
    /// [`LocalEntity::is_unsaved`].
    pub saved: Baseline,
    /// Whether an editor holds an edit of this asset that its bytes do not. The editor
    /// keeps this current through [`Workspace::mark_pending`].
    pending: bool,
    /// Whether this is on this computer, as opposed to a view of a slot.
    ///
    /// A view is a working copy like any other, edited and sent back the same way, but
    /// it is not in the local list and goes when its tab closes. [`Workspace::keep`]
    /// promotes one, and [`Workspace::close_views`] promotes one that holds changes.
    pub kept: bool,
    /// Distinct for every set of bytes this id has held, so a cache of their decode can
    /// tell when it is stale.
    ///
    /// It is the list revision when the bytes landed, so a rename or a send does not
    /// change it.
    pub stamp: u64,
    /// The slot on the attached instrument that holds these bytes, from
    /// [`crate::device::link`].
    ///
    /// Derived from the scan cache and never persisted: it is recomputed whenever that
    /// cache changes and cleared when the instrument goes. An edit leaves it alone, so an
    /// edited asset still points at the slot it was matched to.
    pub link: Option<(ObjectClass, Location)>,
    /// The last write this app made from this asset. See [`Wrote`] and
    /// [`crate::library::agrees`].
    ///
    /// [`Workspace::forget_writes`] clears it when the instrument goes.
    pub wrote: Option<Wrote>,
    /// What a read of its file found before, while it is unread: its kind, tag, slot
    /// checksum and what it plays, without its bytes. See [`Workspace::remember`].
    pub remembered: Option<Box<Summary>>,
    /// The frame something last needed it in, or 0 for never. See [`Workspace::hurry`].
    seen: std::cell::Cell<u64>,
    /// The tag and kind its name says, from [`crate::browser::tagged`], taken whenever
    /// the name is set so that no frame scans the format table for it.
    by_name: Option<(&'static str, crate::browser::Kind)>,
}

impl LocalEntity {
    fn new(id: u64, name: String, origin: Origin, bytes: Bytes, stamp: u64) -> LocalEntity {
        let decoded = Decoded::of(&bytes);
        let mut held = LocalEntity::undecoded(id, name, origin, bytes, stamp);
        held.decoded(decoded);
        held
    }

    /// An asset whose file nothing has read yet, `len` bytes long. It is
    /// [`VerifyState::Reading`], and its kind is what its name says.
    fn listed(id: u64, name: String, origin: Origin, len: u64, stamp: u64) -> LocalEntity {
        let mut held = LocalEntity::undecoded(id, name, origin, Bytes::default(), stamp);
        held.saved.unread = Some(len);
        held
    }

    /// An asset whose bytes are not decoded yet, as [`VerifyState::Reading`] says. Until
    /// [`LocalEntity::decoded`] it has no decode, no container, and the kind its name
    /// says.
    fn undecoded(id: u64, name: String, origin: Origin, bytes: Bytes, stamp: u64) -> LocalEntity {
        let by_name = crate::browser::tagged(&name);
        LocalEntity {
            id,
            name,
            path: None,
            origin,
            saved: Baseline {
                bytes: bytes.clone(),
                crc32: None,
                stamp,
                file: None,
                bytes_crc: None,
                unread: None,
            },
            bytes,
            entity: None,
            plays: None,
            parse_error: None,
            container: None,
            is_text: false,
            verify: VerifyState::Reading,
            pending: false,
            kept: true,
            stamp,
            link: None,
            wrote: None,
            remembered: None,
            seen: Default::default(),
            by_name,
        }
    }

    /// Whether what it is waits on its bytes being read or decoded. One whose summary is
    /// [`remembered`](LocalEntity::remembered) does not, though it is still
    /// [`unread`](LocalEntity::unread).
    pub fn reading(&self) -> bool {
        matches!(self.verify, VerifyState::Reading)
    }

    /// Whether it holds nothing but a file nothing has read this session: no bytes, no
    /// decode, the length its listing gave, and the kind its name or its summary gives.
    /// Anything that acts on it reads it first.
    pub fn unread(&self) -> bool {
        self.saved.unread.is_some() && self.stamp == self.saved.stamp
    }

    /// Draw as `known` says, and match its slot by the checksum it gives.
    fn know(&mut self, known: Summary) {
        self.saved.crc32 = known.crc32;
        self.verify = VerifyState::Remembered(known.verdict);
        self.remembered = Some(Box::new(known));
    }

    /// Take what its bytes decode to. They are what it was saved as, since nothing can
    /// edit an asset still being read.
    fn decoded(&mut self, decoded: Decoded) {
        self.saved.crc32 = decoded.container.as_ref().map(|held| held.body_crc32);
        self.saved.bytes_crc = decoded.crc;
        self.container = decoded.container;
        self.entity = decoded.entity;
        self.plays = decoded.plays;
        self.parse_error = decoded.parse_error;
        self.verify = decoded.verify;
        self.is_text = decoded.is_text;
    }

    /// An asset whose bytes are the file `file` holds, left there and read by range. It
    /// decodes nothing, and its checksum is [`VerifyState::Checking`] until
    /// [`Workspace::poll`] has checked it.
    fn resting(
        id: u64,
        name: String,
        origin: Origin,
        file: Arc<OnDisk>,
        stamp: u64,
    ) -> LocalEntity {
        let by_name = crate::browser::tagged(&name);
        LocalEntity {
            id,
            name,
            path: None,
            origin,
            bytes: Bytes::default(),
            entity: None,
            plays: None,
            parse_error: None,
            container: None,
            is_text: false,
            verify: VerifyState::Checking,
            saved: Baseline::on_disk(file, stamp),
            pending: false,
            kept: true,
            stamp,
            link: None,
            wrote: None,
            remembered: None,
            seen: Default::default(),
            by_name,
        }
    }

    /// The file holding this asset's bytes, while they are the saved ones and are left
    /// there: [`LocalEntity::bytes`] is then empty.
    pub fn rests(&self) -> Option<&Arc<OnDisk>> {
        self.saved
            .file
            .as_ref()
            .filter(|_| self.stamp == self.saved.stamp)
    }

    /// Whether this is as long as `bytes` but not read far enough to tell whether it holds
    /// them: unread, or resting in a file whose check has not taken its CRC yet.
    pub fn might_hold(&self, bytes: &[u8]) -> bool {
        if self.size() != bytes.len() as u64 {
            return false;
        }
        match self.rests() {
            Some(file) => {
                file.known_crc().is_none() && matches!(self.verify, VerifyState::Checking)
            }
            None => self.unread() && !matches!(self.verify, VerifyState::NotRead(_)),
        }
    }

    /// The index of the file this asset rests in, unless its check failed.
    pub fn indexed(&self) -> Option<&ondisk::Index> {
        let file = self.rests()?;
        (!matches!(self.verify, VerifyState::Failed(_))).then_some(&file.index)
    }

    /// How many bytes the asset is.
    pub fn size(&self) -> u64 {
        match (self.rests(), self.unread()) {
            (Some(file), _) => file.len,
            (None, true) => self.saved.size(),
            (None, false) => self.bytes.len() as u64,
        }
    }

    /// The whole body: the bytes held, or a read of the file it rests in.
    ///
    /// ⚠️ A read of a file reads all of it, hundreds of megabytes for a piano library, on
    /// the calling thread, and the browser refuses it. A send reads the file a chunk at a
    /// time, and a copy of it is made by the library, so neither asks.
    pub fn whole(&self) -> std::io::Result<Cow<'_, [u8]>> {
        if self.unread() {
            return Err(std::io::Error::other(UNREAD));
        }
        match self.rests() {
            Some(file) => file.whole().map(Cow::Owned),
            None => Ok(Cow::Borrowed(&self.bytes)),
        }
    }

    /// Whether the instrument can be sent this, and why not: bytes that are not what they
    /// claim to be must never reach a delete-then-write.
    pub fn sendable(&self) -> Result<(), String> {
        if self.unread() {
            return Err(UNREAD.to_string());
        }
        let Some(file) = self.rests() else {
            return nord_usb::envelope::unwrap(&self.bytes)
                .map(|_| ())
                .map_err(|e| e.to_string());
        };
        match (&self.verify, &self.container) {
            (VerifyState::Checked, Some(container)) if container.body_len() > 0 => Ok(()),
            (VerifyState::Checked, _) => {
                Err("the file is a bare CBIN header with no body to send".into())
            }
            (VerifyState::Checking, _) => {
                Err(format!("its {} bytes are still being checked", file.len))
            }
            (state, _) => Err(state.detail()),
        }
    }

    /// How many bytes it holds whole in memory: its bytes, and its baseline's where they
    /// are not the same allocation.
    pub fn held_whole(&self) -> u64 {
        let saved = match self.saved.bytes.shares(&self.bytes) {
            true => 0,
            false => self.saved.bytes.len(),
        };
        (self.bytes.len() + saved) as u64
    }

    /// Whether it holds `bytes`. Never, while it is unread.
    fn holds(&self, bytes: &[u8]) -> bool {
        match self.rests() {
            Some(file) => file.holds(bytes),
            None => !self.unread() && self.bytes == bytes,
        }
    }

    /// The extension its bytes call for. See `format_tag`. One not read yet takes the
    /// one its name carries.
    pub fn format_tag(&self) -> String {
        match (self.rests(), self.listed_tag()) {
            (Some(file), _) => file.index.tag().to_string(),
            (None, Some(tag)) => tag.to_string(),
            (None, None) => format_tag(&self.bytes),
        }
    }

    /// The tag its name carries, while its bytes are not decoded.
    fn listed_tag(&self) -> Option<&'static str> {
        let undecoded = self.reading() || self.unread();
        let (tag, _) = self.by_name.filter(|_| undecoded)?;
        Some(tag)
    }

    /// The tag and kind its name says, as [`crate::browser::tagged`] gives them.
    pub fn by_name(&self) -> Option<(&'static str, crate::browser::Kind)> {
        self.by_name
    }

    /// Name it, and take what the new name says it is.
    fn set_name(&mut self, name: String) {
        self.by_name = crate::browser::tagged(&name);
        self.name = name;
    }

    /// Whether it holds something other than what it was last saved as, an editor's
    /// pending edit included.
    ///
    /// ⚠️ Compares two stamps, not two bodies: every listed row and every frame of the
    /// header ask this, and a piano library is hundreds of megabytes. The stamps are
    /// settled wherever a baseline moves. See [`Baseline::stamp`].
    pub fn is_unsaved(&self) -> bool {
        self.pending || self.stamp != self.saved.stamp
    }

    /// The current bytes as a baseline, which saving adopts.
    fn baseline(&self) -> Baseline {
        Baseline {
            bytes: self.bytes.clone(),
            crc32: self.container.as_ref().map(|held| held.body_crc32),
            stamp: self.stamp,
            file: None,
            bytes_crc: None,
            unread: None,
        }
    }

    /// The slot this asset stands for: its link, or else the slot it came off.
    pub fn spot(&self) -> Option<(ObjectClass, Location)> {
        self.link.or_else(|| self.origin.slot())
    }

    /// A sample instrument's generation, `v2`, `v3` or `v4`, as
    /// [`nord_format::Sample::generation`] spells it, from its decode, the index of the
    /// file it rests in, or what a read of it found.
    pub fn generation(&self) -> Option<&'static str> {
        match (
            self.entity.as_deref(),
            self.rests(),
            self.remembered.as_deref(),
        ) {
            (Some(Entity::Sample(sample)), _, _) => Some(sample.generation()),
            (Some(_), _, _) => None,
            (None, Some(file), _) => match &file.index {
                ondisk::Index::Sample(index) => Some(index.outline().generation()),
                ondisk::Index::Piano(_) => None,
            },
            (None, None, Some(known)) => known.generation,
            (None, None, None) => None,
        }
    }

    /// The format tag, from the decode if there is one and from the container otherwise.
    ///
    /// A note has neither, so its tag comes from its bytes being text. See
    /// `document::text::is_text`.
    pub fn tag(&self) -> String {
        self.held_tag().into_owned()
    }

    /// [`LocalEntity::tag`], borrowed where it can be.
    fn held_tag(&self) -> Cow<'_, str> {
        match (self.entity.as_deref(), &self.container) {
            (Some(entity), _) => Cow::Borrowed(entity.identity().format),
            (None, Some(container)) => Cow::Owned(container.tag()),
            (None, None) if self.is_text => Cow::Borrowed(crate::document::text::EXTENSION),
            (None, None) => match (self.rests(), self.remembered.as_deref()) {
                (Some(file), _) => Cow::Borrowed(file.index.tag()),
                (None, Some(known)) => Cow::Borrowed(&known.tag),
                (None, None) => Cow::Borrowed(self.listed_tag().unwrap_or("?")),
            },
        }
    }

    /// The family whose files carry its [tag](LocalEntity::tag), as
    /// [`accept::Family::of_tag`] says.
    pub fn family(&self) -> Option<accept::Family> {
        family_of(&self.held_tag())
    }

    /// The bytes the wire would carry: the file with its container stripped.
    ///
    /// The counterpart of `nord … get --body` pointed at a file. `None` for anything
    /// that is not a CBIN container.
    pub fn raw_body(&self) -> Option<Vec<u8>> {
        wire_body(&self.bytes)
    }
}

/// Why an asset not read yet has no bytes to hand over.
const UNREAD: &str = "it has not been read yet";

/// [`accept::Family::of_tag`], which scans a table, remembered by tag: every listed row
/// asks it whenever the list changes.
fn family_of(tag: &str) -> Option<accept::Family> {
    thread_local! {
        static FAMILIES: std::cell::RefCell<std::collections::HashMap<String, Option<accept::Family>>> =
            Default::default();
    }
    FAMILIES.with(|known| {
        if let Some(family) = known.borrow().get(tag) {
            return *family;
        }
        let family = accept::Family::of_tag(tag);
        known.borrow_mut().insert(tag.to_string(), family);
        family
    })
}

/// A file's body as the wire carries it, without its container, or `None` for bytes no
/// container this app unwraps.
pub(crate) fn wire_body(bytes: &[u8]) -> Option<Vec<u8>> {
    nord_usb::envelope::unwrap(bytes)
        .ok()
        .map(|read| read.body.0)
}

/// The offset of the first byte where the two differ, if they differ.
pub(crate) fn first_difference(a: &[u8], b: &[u8]) -> Option<usize> {
    if let Some(at) = a.iter().zip(b).position(|(a, b)| a != b) {
        return Some(at);
    }
    // One is a prefix of the other; the first difference is where the shorter one ends.
    (a.len() != b.len()).then(|| a.len().min(b.len()))
}

/// Whether an asset holds something no other copy of it does.
///
/// This decides what happens to a view when its last tab closes: **an unsaved or owed
/// view is precious, and a saved one is disposable.** A saved view holds the slot's
/// bytes, which the instrument still has; an unsaved one is the only copy.
pub fn precious(entity: &LocalEntity, queue: &Queue) -> bool {
    entity.is_unsaved() || queue.holds(entity.id)
}

/// The filename an export suggests for a name: made path-safe, and given the extension
/// the bytes call for unless the name already carries one.
fn export_filename(name: &str, bytes: &[u8]) -> String {
    tagged_filename(name, || format_tag(bytes))
}

/// [`export_filename`] with the extension `tag` gives.
fn tagged_filename(name: &str, tag: impl FnOnce() -> String) -> String {
    let stem = filename_stem(name);
    match crate::strings::carries_tag(&stem) {
        true => stem,
        false => format!("{stem}.{}", tag()),
    }
}

/// The filename a kept asset's file takes when the app chooses it: the name made one the
/// library's rule allows, and given the extension the bytes call for unless it already
/// carries one.
pub(crate) fn library_filename(entity: &LocalEntity) -> String {
    let stem = names::portable(&entity.name);
    match crate::strings::carries_tag(&stem) {
        true => stem,
        false => names::portable(&format!("{stem}.{}", entity.format_tag())),
    }
}

/// The filename for a decoded zone's WAV: the instrument's name made path-safe, and the
/// zone numbered as the document numbers it.
///
/// `nord sample decode --out` writes the same name, so a zone exported from either tool
/// gets one name.
pub fn zone_wav_name(instrument: &str, zone: usize) -> String {
    let stem = filename_stem(instrument);
    format!("{stem}-zone{zone}.wav")
}

/// The filename for a decoded piano stroke's WAV: the library's name made path-safe,
/// and the stroke named as `nord piano decode` names it, `<root>-b<bank>-l<layer>`, with
/// the MIDI note zero-padded to three digits.
pub fn stroke_wav_name(library: &str, root: u8, bank: u8, layer: u8) -> String {
    let stem = filename_stem(library);
    format!("{stem}-{root:03}-b{bank}-l{layer:02}.wav")
}

/// A name reduced to what a path can carry: runs of whitespace, dashes, and path
/// separators become one `-`, control characters are dropped, and leading or trailing
/// dots and dashes are trimmed so the file is neither hidden nor option-like. A name
/// with nothing left is `unnamed`. For filenames only; the name itself is never changed.
fn filename_stem(label: &str) -> String {
    // A separator is written only before the next kept character, so a run collapses to
    // one `-` and none trails.
    let mut owed = false;
    let mut out = String::with_capacity(label.len());
    for c in label.chars() {
        match c {
            '-' => owed = !out.is_empty(),
            _ if c.is_whitespace() => owed = !out.is_empty(),
            // Path separators, plus the characters a Windows filename cannot hold.
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => owed = !out.is_empty(),
            _ if c.is_control() => {}
            _ => {
                if std::mem::take(&mut owed) {
                    out.push('-');
                }
                out.push(c);
            }
        }
    }
    // A leading dot hides the file and dots alone spell `.` and `..`; a leading dash is
    // an option to every tool that later reads it.
    match out.trim_matches(['.', '-']) {
        "" => "unnamed".to_string(),
        stem => stem.to_string(),
    }
}

/// The extension an export gets when the name carries none: the CBIN tag in the bytes,
/// the project format's name, the note extension for text, or `bin`.
fn format_tag(bytes: &[u8]) -> String {
    // ⚠️ Offset 8 means something only after the CBIN magic. A text format can hold
    // alphanumerics there by accident.
    if bytes.starts_with(nord_format::cbin::MAGIC) {
        return bytes
            .get(8..12)
            .filter(|tag| tag.iter().all(|b| b.is_ascii_alphanumeric()))
            .map(|tag| String::from_utf8_lossy(tag).into_owned())
            .unwrap_or_else(|| "bin".to_string());
    }
    if bytes.starts_with(nsmpproj::MAGIC) {
        return nsmpproj::FORMAT.to_string();
    }
    if crate::document::text::is_text(bytes) {
        return crate::document::text::EXTENSION.to_string();
    }
    "bin".to_string()
}

/// Re-encode and compare, the same check `nord verify` runs on a file.
fn verify(entity: &Entity, bytes: &[u8]) -> VerifyState {
    if matches!(entity, Entity::Bundle(_)) {
        return VerifyState::NotApplicable("a bundle is an archive; it does not re-encode");
    }
    let out = match nord_format::to_bytes(entity) {
        Ok(out) => out,
        Err(e) => return VerifyState::Failed(e.to_string()),
    };
    match first_difference(&out, bytes) {
        Some(at) => VerifyState::Differs { at },
        None => VerifyState::Ok,
    }
}

/// One product family and the objects that can be created from nothing for it. The New
/// menu is arranged this way: a family, then a kind inside it.
pub struct Family {
    pub label: &'static str,
    pub kinds: &'static [Fresh],
}

/// An all-zero body is a legal body: each field's type decodes every value of its bits,
/// so nothing is out of range. Build one under the newest version the decoder is
/// validated against.
///
/// ⚠️ Legal is not **default**. The result has every control at zero, which no panel
/// ships with. [`Fresh::zeroed`] lets the menu warn the user before they make one.
macro_rules! zeroed {
    ($body:ty, $len:expr, $format:expr, $versions:expr, $wrap:expr) => {{
        let body = <$body>::try_from([0u8; $len]).map_err(|e| format!("{e}"))?;
        let version = *$versions.last().ok_or("the format knows no version")?;
        $wrap(Cbin {
            header: Header::new($format, (0, 0), version),
            body,
        })
    }};
}

/// The objects the New menu offers, across every format this app can build from
/// nothing.
///
/// ⚠️ Only bodies that **decode** are here, plus the note, which has no body. A stub
/// format, such as the Stage 3's song or any Stage's settings, round-trips its container
/// and nothing more. A zeroed one would be 45 bytes of nothing under a tag, and this app
/// could say nothing true about it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fresh {
    /// The Electro 5's four kinds, each built by `nord-format`'s constructor.
    Program,
    Live,
    SetList,
    Settings,
    Stage2Program,
    Stage3Program,
    Stage3Synth,
    Stage4Program,
    Stage4Organ,
    Stage4Piano,
    Stage4Synth,
    /// An empty note. No instrument holds one.
    Text,
}

impl Fresh {
    pub const ALL: [Fresh; 12] = [
        Fresh::Program,
        Fresh::Live,
        Fresh::SetList,
        Fresh::Settings,
        Fresh::Stage2Program,
        Fresh::Stage3Program,
        Fresh::Stage3Synth,
        Fresh::Stage4Program,
        Fresh::Stage4Organ,
        Fresh::Stage4Piano,
        Fresh::Stage4Synth,
        Fresh::Text,
    ];

    /// The kinds no product family makes, which the New menu offers on their own.
    /// Together with [`Fresh::FAMILIES`] this is every kind, each offered once.
    pub const LOOSE: [Fresh; 1] = [Fresh::Text];

    pub const FAMILIES: [Family; 4] = [
        Family {
            label: "Electro 5",
            kinds: &[Fresh::Program, Fresh::Live, Fresh::SetList, Fresh::Settings],
        },
        Family {
            label: "Stage 2",
            kinds: &[Fresh::Stage2Program],
        },
        Family {
            label: "Stage 3",
            kinds: &[Fresh::Stage3Program, Fresh::Stage3Synth],
        },
        Family {
            label: "Stage 4",
            kinds: &[
                Fresh::Stage4Program,
                Fresh::Stage4Organ,
                Fresh::Stage4Piano,
                Fresh::Stage4Synth,
            ],
        },
    ];

    /// What the kind is called inside its family's menu.
    pub fn label(self) -> &'static str {
        match self {
            Fresh::Program | Fresh::Stage2Program | Fresh::Stage3Program | Fresh::Stage4Program => {
                "Program"
            }
            Fresh::Live => "Live slot",
            Fresh::SetList => "Set list",
            Fresh::Settings => "Settings",
            Fresh::Stage3Synth | Fresh::Stage4Synth => "Synth preset",
            Fresh::Stage4Organ => "Organ preset",
            Fresh::Stage4Piano => "Piano preset",
            Fresh::Text => "Text note",
        }
    }

    pub fn tag(self) -> &'static str {
        match self {
            Fresh::Program => ne5::program::FORMAT,
            Fresh::Live => ne5::live::FORMAT,
            Fresh::SetList => ne5::song::FORMAT,
            Fresh::Settings => ne5::settings::FORMAT,
            Fresh::Stage2Program => ns2::program::FORMAT,
            Fresh::Stage3Program => ns3::program::FORMAT,
            Fresh::Stage3Synth => ns3::synth::FORMAT,
            Fresh::Stage4Program => ns4::program::FORMAT,
            Fresh::Stage4Organ => ns4::organ_preset::FORMAT,
            Fresh::Stage4Piano => ns4::piano_preset::FORMAT,
            Fresh::Stage4Synth => ns4::synth::FORMAT,
            Fresh::Text => crate::document::text::EXTENSION,
        }
    }

    /// Whether this makes a zeroed body instead of a constructed object. An Electro 5
    /// kind comes from `nord-format`'s constructor, and every Stage kind has every
    /// control at zero.
    pub fn zeroed(self) -> bool {
        match self {
            Fresh::Program | Fresh::Live | Fresh::SetList | Fresh::Settings | Fresh::Text => false,
            Fresh::Stage2Program
            | Fresh::Stage3Program
            | Fresh::Stage3Synth
            | Fresh::Stage4Program
            | Fresh::Stage4Organ
            | Fresh::Stage4Piano
            | Fresh::Stage4Synth => true,
        }
    }

    /// The hover text for the menu entry, where the user would otherwise have to open
    /// the file to find something out.
    pub fn note(self) -> Option<&'static str> {
        match self {
            Fresh::Text => Some(
                "A text file. It stays on this computer, since no instrument has a \
                 folder for one.",
            ),
            _ => self.zeroed().then_some(
                "Every control at zero. The file decodes and re-saves byte for byte, but \
                 it is not a factory program. This app does not know what one would hold.",
            ),
        }
    }

    /// The file this makes, as [`Workspace::create`] adds it to the list.
    pub(crate) fn bytes(self) -> Result<Vec<u8>, String> {
        let at = |slot| ne5::program::Location::new(0, slot).map_err(|e| e.to_string());
        let electro5 = |class| -> Result<Entity, String> {
            // A set list is only four program pointers, so it starts with the first four
            // programs.
            Entity::electro5(class, [at(0)?, at(1)?, at(2)?, at(3)?])
                .ok_or_else(|| format!("no new {} exists", self.label()))?
                .map_err(|e| e.to_string())
        };
        let entity = match self {
            Fresh::Program => electro5(Slot::Program)?,
            Fresh::Live => electro5(Slot::Live)?,
            Fresh::SetList => electro5(Slot::SetList)?,
            Fresh::Settings => electro5(Slot::Settings)?,
            Fresh::Stage2Program => zeroed!(
                ns2::Program,
                ns2::program::BODY_LEN,
                ns2::program::FORMAT,
                ns2::program::KNOWN_VERSIONS,
                |f| Entity::Program(Program::Stage2(f))
            ),
            Fresh::Stage3Program => zeroed!(
                ns3::Program,
                ns3::program::BODY_LEN,
                ns3::program::FORMAT,
                ns3::program::KNOWN_VERSIONS,
                |f| Entity::Program(Program::Stage3(f))
            ),
            Fresh::Stage3Synth => zeroed!(
                ns3::SynthPreset,
                ns3::synth::BODY_LEN,
                ns3::synth::FORMAT,
                ns3::synth::KNOWN_VERSIONS,
                |f| Entity::Synth(Synth::Stage3(f))
            ),
            Fresh::Stage4Program => zeroed!(
                ns4::Program,
                ns4::program::BODY_LEN,
                ns4::program::FORMAT,
                ns4::program::KNOWN_VERSIONS,
                |f| Entity::Program(Program::Stage4(f))
            ),
            Fresh::Stage4Organ => zeroed!(
                ns4::organ_preset::OrganPreset,
                ns4::organ_preset::BODY_LEN,
                ns4::organ_preset::FORMAT,
                ns4::organ_preset::KNOWN_VERSIONS,
                |f| Entity::OrganPreset(OrganPreset::Stage4(f))
            ),
            Fresh::Stage4Piano => zeroed!(
                ns4::piano_preset::PianoPreset,
                ns4::piano_preset::BODY_LEN,
                ns4::piano_preset::FORMAT,
                ns4::piano_preset::KNOWN_VERSIONS,
                |f| Entity::PianoPreset(PianoPreset::Stage4(f))
            ),
            Fresh::Stage4Synth => zeroed!(
                ns4::synth::SynthPreset,
                ns4::synth::BODY_LEN,
                ns4::synth::FORMAT,
                ns4::synth::KNOWN_VERSIONS,
                |f| Entity::Synth(Synth::Stage4(f))
            ),
            // A new note is an empty file, with nothing to encode.
            Fresh::Text => return Ok(Vec::new()),
        };
        nord_format::to_bytes(&entity).map_err(|e| e.to_string())
    }
}

/// What the decode made of arriving bytes, for the status line. The details go to the
/// log either way.
enum Arrival {
    Read,
    /// It decoded, but it does not re-encode to the bytes it came from.
    Unverified,
    Unreadable,
}

/// One asset as a store holds it.
pub struct Saved {
    pub id: u64,
    pub name: String,
    pub path: Option<LibPath>,
    pub origin: Origin,
    /// What it was last saved as. Empty where `file` holds it, or where it is unread.
    pub saved: Vec<u8>,
    /// The file holding what it was last saved as, left there and read by range.
    pub file: Option<Arc<OnDisk>>,
    /// The length of the file holding what it was last saved as, where nothing has read
    /// that file yet.
    pub unread: Option<u64>,
    /// What it holds now, if that differs from what it was saved as.
    pub unsaved: Option<Vec<u8>>,
}

/// What a background task hands back to the UI thread.
enum Incoming {
    Opened {
        name: String,
        bytes: Vec<u8>,
    },
    /// A file File ▸ Open… picked, to be copied into the library.
    Picked(Outside),
    /// Every file one pick of WAVs returned, together, because the draft asks one
    /// question about the whole set.
    Wavs {
        making: Making,
        files: Vec<(String, Vec<u8>)>,
    },
    /// Every demo file, fetched; see [`crate::demo`].
    Demos(Vec<(String, Vec<u8>)>),
    Note(String),
    Failed(String),
}

/// Something taken from the listed assets, and the revision and layout it was taken at.
#[derive(Default)]
struct Taken<T> {
    at: Option<(u64, u64)>,
    value: T,
}

/// An asset a copy or an overwrite came from, with the stamp of what it held then
/// ([`Workspace::unchanged`]). It goes once what was made of it has landed.
#[derive(Clone, Copy, Debug)]
pub struct Leaving {
    pub from: u64,
    edit: u64,
}

pub struct Workspace {
    entities: Vec<LocalEntity>,
    /// Where each id sits in `entities`, as of the [`Workspace::layout`] it was built at.
    at: std::cell::RefCell<(u64, std::collections::HashMap<u64, usize>)>,
    next_id: u64,
    /// Bumped by every change to the list, so the shell can tell when the store is
    /// behind without comparing every asset's bytes.
    revision: u64,
    /// Bumped by every change to which assets are listed, or to their names and paths.
    layout: u64,
    families: std::cell::RefCell<Taken<Vec<accept::Family>>>,
    generations: std::cell::RefCell<Taken<Vec<&'static str>>>,
    kinds: std::cell::RefCell<Taken<Vec<crate::browser::Kind>>>,
    /// How many times [`Workspace::families_present`] has been taken.
    #[cfg(test)]
    pub(crate) families_taken: std::cell::Cell<usize>,
    /// How many times [`Workspace::kinds`] has been taken.
    #[cfg(test)]
    pub(crate) kinds_taken: std::cell::Cell<usize>,
    /// How many times the list has been searched end to end for an id.
    #[cfg(test)]
    pub(crate) searched: std::cell::Cell<usize>,
    ctx: egui::Context,
    tx: Sender<Incoming>,
    rx: Receiver<Incoming>,
    /// The WAVs a New pick came back with, waiting on their root keys. See
    /// [`crate::newproject`].
    draft: Option<Draft>,
    /// Demo files fetched and waiting to be filed, which takes the folders this list
    /// does not hold.
    demos: Option<Vec<(String, Vec<u8>)>>,
    /// Assets resting in their files whose checksums are still to be checked, one at a
    /// time, in the order they arrived.
    checks: VecDeque<(u64, Arc<OnDisk>)>,
    checking: Option<Check>,
    /// Assets whose file is being copied in, each with what it is a copy of. Each is
    /// unread until its copy lands.
    arriving: std::collections::BTreeMap<u64, CopyOf>,
    /// The asset each arriving copy moves over the file of another comes from, by the
    /// asset it lands on. It goes only once its copy has landed.
    moving_over: std::collections::BTreeMap<u64, Leaving>,
    /// The asset each overwrite with bytes held whole comes from, by the asset it
    /// overwrites, with the stamp of the save that writes them. It goes only once that
    /// save has landed.
    saving_over: std::collections::BTreeMap<u64, (Leaving, u64)>,
    /// The files File ▸ Open… picked, not yet taken.
    picked: Vec<Outside>,
    /// Assets whose bytes are still to be decoded, in the order they arrived, and those
    /// asked for first.
    undecoded: VecDeque<u64>,
    hurried: std::cell::RefCell<std::collections::BTreeSet<u64>>,
    /// Unread assets something needs, each with how much, not yet asked of the library,
    /// and those asked and not yet answered.
    wanted: std::cell::RefCell<std::collections::BTreeMap<u64, Need>>,
    asked: std::collections::BTreeSet<u64>,
    /// Decodes running off the frame, each of a few assets, and every asset in them.
    decoding: Vec<Decode>,
    flying: std::collections::BTreeSet<u64>,
    /// The edit an editor made of each asset resting in its file, made over the file it
    /// rests in. See [`Workspace::hold_edit`].
    edits: std::collections::BTreeMap<u64, Held>,
    /// Counts the calls to [`Workspace::poll`], one a frame, from 1.
    frame: u64,
}

/// An edit of an asset resting in its file, and how far a save of it has got.
struct Held {
    edit: Edit,
    /// Moves whenever the edit does.
    stamp: u64,
    /// The file the edit was last made over, `None` where the asset rests in none.
    over: Option<Arc<OnDisk>>,
    /// The rewrite that writes the edit into `over`, or why it does not apply there.
    rewrite: Result<Arc<Rewrite>, String>,
    save: Save,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Save {
    No,
    /// The store has still to send it.
    Asked,
    /// The store has sent the edit of this stamp and not heard back.
    Sent(u64),
}

/// Assets decoded off the frame, each answered with the stamp of the bytes decoded.
struct Decode {
    ids: Vec<u64>,
    job: work::Job<Vec<(u64, u64, Decoded)>>,
}

/// How many decodes run at once, and how many assets and bytes one takes. The browser
/// has one thread, so there one runs inline each frame, and takes less.
#[cfg(not(target_arch = "wasm32"))]
const DECODES: usize = 8;
#[cfg(not(target_arch = "wasm32"))]
const DECODE: (usize, u64) = (64, 16 << 20);
#[cfg(target_arch = "wasm32")]
const DECODES: usize = 1;
#[cfg(target_arch = "wasm32")]
const DECODE: (usize, u64) = (16, 2 << 20);

/// How much something needs an unread asset read, least first: the library reads what
/// is needed most first.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Need {
    /// A row in view, which draws what the read finds.
    InView,
    Selected,
    /// It is open, or acted on.
    Now,
}

/// How many files one read asks of the library at most, and how many of their bytes, so
/// a read something waits on goes in behind no more than this.
const READ: (usize, u64) = (32, 8 << 20);

/// A file's checksum being checked off the frame.
struct Check {
    id: u64,
    file: Arc<OnDisk>,
    job: work::Job<Result<ondisk::Sums, String>>,
}

impl Workspace {
    pub fn new(ctx: egui::Context) -> Workspace {
        let (tx, rx) = std::sync::mpsc::channel();
        Workspace {
            entities: Vec::new(),
            at: Default::default(),
            families: Default::default(),
            generations: Default::default(),
            kinds: Default::default(),
            #[cfg(test)]
            families_taken: Default::default(),
            #[cfg(test)]
            kinds_taken: Default::default(),
            #[cfg(test)]
            searched: Default::default(),
            next_id: 1,
            revision: 0,
            layout: 1,
            ctx,
            tx,
            rx,
            draft: None,
            demos: None,
            checks: VecDeque::new(),
            checking: None,
            arriving: Default::default(),
            moving_over: Default::default(),
            saving_over: Default::default(),
            picked: Vec::new(),
            undecoded: VecDeque::new(),
            hurried: Default::default(),
            wanted: Default::default(),
            asked: Default::default(),
            decoding: Vec::new(),
            flying: Default::default(),
            edits: Default::default(),
            frame: 1,
        }
    }

    /// The context the app draws in, for acts that need the window and not the list.
    pub fn ctx(&self) -> &egui::Context {
        &self.ctx
    }

    /// Counts changes to the list, not to any one asset.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Counts changes to which assets are listed and to their names and paths, so a view
    /// of where they are is taken again only when it would differ.
    pub fn layout(&self) -> u64 {
        self.layout
    }

    /// The families of the listed assets, in [`accept::Family::ALL`] order.
    ///
    /// Files that name no family (the shared library formats, the carriers, bytes that did
    /// not decode) add none, so a list of samples spans no families. Taken again only
    /// when the list or one of its assets changes, so a frame that draws a family word
    /// per row does not take it per row.
    pub fn families_present(&self) -> Vec<accept::Family> {
        self.taken(&self.families, || {
            #[cfg(test)]
            self.families_taken.set(self.families_taken.get() + 1);
            let here: std::collections::HashSet<accept::Family> =
                self.listed().filter_map(LocalEntity::family).collect();
            accept::Family::ALL
                .into_iter()
                .filter(|family| here.contains(family))
                .collect()
        })
    }

    /// The sample generations of the listed assets, sorted. Taken again only when the
    /// list or one of its assets changes, as the families are.
    pub fn generations_present(&self) -> Vec<&'static str> {
        self.taken(&self.generations, || {
            let mut here: Vec<&'static str> =
                self.listed().filter_map(LocalEntity::generation).collect();
            here.sort_unstable();
            here.dedup();
            here
        })
    }

    /// The kinds of the listed assets, in [`crate::browser::Kind::ALL`] order. Taken again
    /// only when the list or one of its assets changes, as the families are.
    pub fn kinds(&self) -> Vec<crate::browser::Kind> {
        use crate::browser::Kind;
        self.taken(&self.kinds, || {
            #[cfg(test)]
            self.kinds_taken.set(self.kinds_taken.get() + 1);
            let mut here = Vec::new();
            for kind in self.listed().map(Kind::of) {
                if !here.contains(&kind) {
                    here.push(kind);
                }
            }
            Kind::ALL
                .into_iter()
                .filter(|kind| here.contains(kind))
                .collect()
        })
    }

    /// What `held` last took, or `take` again where the list has changed since.
    fn taken<T: Clone>(&self, held: &std::cell::RefCell<Taken<T>>, take: impl FnOnce() -> T) -> T {
        let now = Some((self.revision, self.layout));
        let mut held = held.borrow_mut();
        if held.at != now {
            *held = Taken {
                at: now,
                value: take(),
            };
        }
        held.value.clone()
    }

    /// Record a change to which assets are held, or to their names or paths.
    fn moved(&mut self) {
        self.revision += 1;
        self.layout += 1;
    }

    /// Where each id sits in the list, taken again where the layout moved since.
    fn positions(&self) -> std::cell::RefMut<'_, (u64, std::collections::HashMap<u64, usize>)> {
        let mut at = self.at.borrow_mut();
        if at.0 != self.layout {
            #[cfg(test)]
            self.searched.set(self.searched.get() + 1);
            let positions = self.entities.iter().enumerate();
            *at = (
                self.layout,
                positions
                    .map(|(position, entity)| (entity.id, position))
                    .collect(),
            );
        }
        at
    }

    /// Where `id` sits in the list.
    fn position(&self, id: u64) -> Option<usize> {
        let at = self.positions();
        // A change to the list that did not bump the layout is caught here: the list is
        // searched rather than another asset handed back.
        match at.1.get(&id) {
            Some(&position)
                if self
                    .entities
                    .get(position)
                    .is_some_and(|held| held.id == id) =>
            {
                Some(position)
            }
            _ => {
                #[cfg(test)]
                self.searched.set(self.searched.get() + 1);
                self.entities.iter().position(|held| held.id == id)
            }
        }
    }

    fn get_mut(&mut self, id: u64) -> Option<&mut LocalEntity> {
        let position = self.position(id)?;
        self.entities.get_mut(position)
    }

    /// Everything held in memory, views included. The local **list** is
    /// [`Workspace::listed`].
    pub fn entities(&self) -> &[LocalEntity] {
        &self.entities
    }

    /// What "This computer" shows: everything except the views of a slot.
    pub fn listed(&self) -> impl Iterator<Item = &LocalEntity> {
        self.entities.iter().filter(|e| e.kept)
    }

    pub fn get(&self, id: u64) -> Option<&LocalEntity> {
        self.entities.get(self.position(id)?)
    }

    /// Whether this is a view of a slot and not on this computer.
    pub fn is_view(&self, id: u64) -> bool {
        self.get(id).is_some_and(|entity| !entity.kept)
    }

    /// Take a copy of a slot without putting it in the local list.
    ///
    /// A double-click on a slot opens one: a tab and a document over a working copy,
    /// which is edited and sent back like any other and goes when its tab closes.
    pub fn view(&mut self, name: String, origin: Origin, bytes: Vec<u8>, log: &mut Log) -> u64 {
        let (id, _) = self.add(name, origin, bytes, log);
        let Some(entity) = self.get_mut(id) else {
            return id;
        };
        entity.kept = false;
        let where_ = match entity.origin.slot() {
            Some((class, at)) => crate::strings::place(class, at),
            None => "the instrument".to_string(),
        };
        log.say(format!(
            "Viewing {where_}. “{}” is not kept on this computer.",
            entity.name
        ));
        id
    }

    /// The view of a slot, if one is open. A slot has at most one: a second would be two
    /// working copies of one place, editable apart and both sendable back to it.
    pub fn view_of(&self, class: ObjectClass, at: Location) -> Option<u64> {
        self.entities
            .iter()
            .find(|e| !e.kept && e.origin.slot() == Some((class, at)))
            .map(|e| e.id)
    }

    /// Promote a view into the local list, with its edits and its place in the queue.
    pub fn keep(&mut self, id: u64, log: &mut Log) {
        let Some(entity) = self.get_mut(id) else {
            return;
        };
        if std::mem::replace(&mut entity.kept, true) {
            return;
        }
        let name = entity.name.clone();
        self.moved();
        log.say(format!("“{name}” is on this computer."));
    }

    /// Drop the views no open tab shows anymore, except those holding changes.
    ///
    /// A view lives as long as its tab. Nothing lists it, so a view left behind could
    /// never be reached or removed.
    ///
    /// ⚠️ **A view is the only copy of what it holds.** Nothing lists it and the store
    /// skips it, so closing its tab is the only gesture in this app that can destroy an
    /// edit, and the × sits beside the badge saying the edit is owed to a slot. An edited
    /// or owed view is promoted into the list instead, and only an untouched one is
    /// dropped.
    pub fn close_views(&mut self, open: impl Fn(u64) -> bool, queue: &Queue, log: &mut Log) {
        let mut rescued = Vec::new();
        let before = self.entities.len();
        self.entities.retain_mut(|entity| {
            if entity.kept || open(entity.id) {
                return true;
            }
            if !precious(entity, queue) {
                return false;
            }
            entity.kept = true;
            rescued.push(entity.name.clone());
            true
        });
        for name in &rescued {
            log.say(format!(
                "“{name}” is kept on this computer because it has changes the instrument \
                 does not."
            ));
        }
        if self.entities.len() == before && rescued.is_empty() {
            return;
        }
        self.moved();
    }

    /// Recompute every asset's link. Call whenever the instrument's scan cache changes,
    /// with [`crate::device::link`] as `held_by`. Returns whether any link moved.
    pub fn relink(
        &mut self,
        held_by: impl Fn(&LocalEntity) -> Option<(ObjectClass, Location)>,
    ) -> bool {
        let mut moved = false;
        for entity in &mut self.entities {
            let link = held_by(entity);
            moved |= entity.link != link;
            entity.link = link;
        }
        moved
    }

    /// Rename an asset held here. Nothing leaves this computer.
    pub fn rename(&mut self, id: u64, name: String) {
        if let Some(entity) = self.get_mut(id) {
            entity.set_name(name);
            self.moved();
        }
    }

    /// Put an asset's file at `path`, which also names it.
    pub fn place(&mut self, id: u64, path: LibPath) {
        if let Some(entity) = self.get_mut(id) {
            entity.set_name(path.leaf().to_string());
            entity.path = Some(path);
            self.moved();
        }
    }

    /// Forget where an asset's file was to go, so it is placed again under a free name.
    pub fn unplace(&mut self, id: u64) {
        if let Some(entity) = self.get_mut(id) {
            entity.path = None;
            self.moved();
        }
    }

    /// Move every asset under the folder `from` to the same place under `to`.
    pub fn relocate(&mut self, from: &LibPath, to: &LibPath) {
        for entity in &mut self.entities {
            if let Some(moved) = entity.path.as_ref().and_then(|at| at.moved(from, to)) {
                entity.set_name(moved.leaf().to_string());
                entity.path = Some(moved);
            }
        }
        self.moved();
    }

    /// Take bytes that were saved over this asset's file from outside the app. They are
    /// both what it holds and what it was saved as.
    ///
    /// ⚠️ Only for an asset with nothing unsaved. Over an unsaved edit, see
    /// [`Workspace::rebase`].
    pub fn adopt(&mut self, id: u64, bytes: Vec<u8>, log: &mut Log) {
        self.replace_bytes(id, bytes, log);
        self.mark_saved(id);
    }

    /// [`Workspace::adopt`] for a file left on disk: the asset rests in it.
    pub fn adopt_file(&mut self, id: u64, file: Arc<OnDisk>) {
        let stamp = self.stamp();
        self.rest(id, file, stamp);
    }

    /// Make `theirs` the saved baseline under an unsaved edit, which stays: the next save
    /// writes the edit over them, and a revert takes them. An asset resting in its file
    /// adopts them instead, and the edit held of it is made again over them.
    pub fn rebase(&mut self, id: u64, theirs: Vec<u8>, log: &mut Log) {
        if !self.apart(id) {
            return self.adopt(id, theirs, log);
        }
        let stamp = self.stamp_for(id, &theirs);
        if let Some(entity) = self.get_mut(id) {
            entity.saved = Baseline::read(theirs.into(), stamp);
        }
    }

    /// [`Workspace::rebase`] onto a file left on disk.
    pub fn rebase_file(&mut self, id: u64, theirs: Arc<OnDisk>) {
        if !self.apart(id) {
            return self.adopt_file(id, theirs);
        }
        let stamp = self.stamp();
        if let Some(entity) = self.get_mut(id) {
            entity.saved = Baseline::on_disk(theirs.clone(), stamp);
        }
        self.check(id, theirs);
    }

    /// Count an asset as unsaved again, because the save it was counted saved by did not
    /// land. One resting in its file holds what the file holds, and an edit held over it
    /// stays unsaved.
    pub fn unsave(&mut self, id: u64) {
        self.edit_not_saved(id);
        if !self.apart(id) {
            return;
        }
        let stamp = self.stamp();
        if let Some(entity) = self.get_mut(id) {
            if !entity.is_unsaved() {
                entity.saved.stamp = stamp;
            }
        }
    }

    /// Whether an asset's bytes stand apart from what it was saved as, so that its saved
    /// baseline can move away from them. One resting in its file holds nothing apart from
    /// the file, and is never read whole to make it so.
    fn apart(&self, id: u64) -> bool {
        self.get(id).is_none_or(|entity| entity.rests().is_none())
    }

    /// Leave an asset's bytes in `file`, which holds them, under `stamp`, and check the
    /// file's checksum off the frame.
    fn rest(&mut self, id: u64, file: Arc<OnDisk>, stamp: u64) {
        self.swap(id, false, |held| {
            LocalEntity::resting(
                id,
                held.name.clone(),
                held.origin.clone(),
                file.clone(),
                stamp,
            )
        });
        self.check(id, file);
        self.follow_edit(id);
    }

    /// Put `made` in place of an asset, keeping what an asset keeps whatever bytes it
    /// holds: where its file is, whether it is kept, its link, its last write and its
    /// pending edit, and its saved baseline where `keep_saved` says so.
    fn swap(&mut self, id: u64, keep_saved: bool, made: impl FnOnce(&LocalEntity) -> LocalEntity) {
        let Some(entity) = self.get_mut(id) else {
            return;
        };
        let made = made(entity);
        let saved = match keep_saved {
            true => std::mem::take(&mut entity.saved),
            false => made.saved,
        };
        *entity = LocalEntity {
            path: entity.path.take(),
            kept: entity.kept,
            link: entity.link,
            wrote: entity.wrote,
            pending: entity.pending,
            seen: entity.seen.clone(),
            saved,
            ..made
        };
        self.revision += 1;
    }

    /// Check the checksum of `file`, which holds an asset's saved bytes, once the checks
    /// before it have answered.
    fn check(&mut self, id: u64, file: Arc<OnDisk>) {
        self.checks.push_back((id, file));
        self.next_check();
    }

    fn next_check(&mut self) {
        if self.checking.is_some() {
            return;
        }
        let Some((id, file)) = self.checks.pop_front() else {
            return;
        };
        let job = file.verify(&self.ctx);
        self.checking = Some(Check { id, file, job });
    }

    /// Fold in the check that has answered, where one has, and start the next.
    fn checked(&mut self, answer: work::Answer<Result<ondisk::Sums, String>>, log: &mut Log) {
        let answer = match answer {
            work::Answer::Running => return,
            work::Answer::Answered(answer) => answer,
            work::Answer::Died => Err("the check stopped without an answer".to_string()),
        };
        let Some(Check { id, file, .. }) = self.checking.take() else {
            return;
        };
        let answer = answer.and_then(|sums| Container::of_file(&file, sums));
        self.next_check();
        let Some(entity) = self.get_mut(id) else {
            return;
        };
        // An answer about a file the asset no longer stands on is dropped.
        if !entity
            .saved
            .file
            .as_ref()
            .is_some_and(|held| Arc::ptr_eq(held, &file))
        {
            return;
        }
        let verify = match &answer {
            Ok(container) if container.checksum_ok => VerifyState::Checked,
            Ok(_) => VerifyState::Failed("the stored checksum does not match the body".into()),
            Err(why) => VerifyState::Failed(why.clone()),
        };
        entity.saved.crc32 = match &verify {
            VerifyState::Checked => answer.as_ref().ok().map(|held| held.body_crc32),
            _ => None,
        };
        let name = entity.name.clone();
        match &verify {
            VerifyState::Checked => log.info(format!(
                "{name}: {} ({} bytes), checksum ok",
                file.index.tag(),
                file.len
            )),
            other => log.warn(format!(
                "{name}: verify {}: {}",
                other.badge(),
                other.detail()
            )),
        }
        // An edit made while the file was checked has a decode and a verify of its own.
        if entity.rests().is_some() {
            entity.parse_error = match &verify {
                VerifyState::Failed(why) => Some(why.clone()),
                _ => None,
            };
            entity.container = answer.ok();
            entity.verify = verify;
        }
        self.revision += 1;
    }

    /// How many assets something needs that are still being read or decoded.
    pub fn reading(&self) -> usize {
        let undecoded = |entity: &LocalEntity| entity.reading() && !entity.unread();
        let decoding = self.entities.iter().filter(|entity| undecoded(entity));
        let asked = self.asked.iter().filter_map(|id| self.get(*id));
        let asked = asked.filter(|entity| !undecoded(entity));
        decoding.count() + asked.count() + self.wanted.borrow().len()
    }

    /// Something needs `id` whole: it is open, or acted on. One not read yet is asked of
    /// the library ahead of everything else, and one not decoded yet is decoded ahead of
    /// the others.
    /// Either way it is needed now, and is not let go for room until a frame passes
    /// without it being needed.
    pub fn hurry(&self, id: u64) {
        self.need(id, Need::Now);
    }

    /// Something needs `id` whole because it is selected: as [`Workspace::hurry`], read
    /// after what is open or acted on.
    pub fn selected(&self, id: u64) {
        self.need(id, Need::Selected);
    }

    /// Each of these assets is a row a list draws. A row draws what a summary remembers,
    /// so only one with nothing remembered is read for it, after what is open, acted on
    /// or selected, and only while it is drawn; otherwise as [`Workspace::hurry`].
    pub fn in_view(&self, ids: impl IntoIterator<Item = u64>) {
        for id in ids {
            self.need(id, Need::InView);
        }
    }

    fn need(&self, id: u64, need: Need) {
        let Some(entity) = self.get(id) else {
            return;
        };
        if self.arriving.contains_key(&id) {
            return;
        }
        entity.seen.set(self.frame);
        let whole = need != Need::InView;
        let remembered = matches!(entity.verify, VerifyState::Remembered(_));
        if !entity.reading() && !(whole && remembered) {
            return;
        }
        if !entity.unread() {
            self.hurried.borrow_mut().insert(id);
            return;
        }
        if self.asked.contains(&id) {
            return;
        }
        let mut wanted = self.wanted.borrow_mut();
        let held = wanted.get(&id).copied();
        if held.is_none_or(|held| held < need) {
            wanted.insert(id, need);
            self.ctx.request_repaint();
        }
    }

    /// The unread assets the library is now asked for: those needed most, a read's worth
    /// of them. A row no longer drawn is not read for it.
    pub fn take_wanted(&mut self) -> Vec<u64> {
        let mut wanted = self.wanted.borrow_mut();
        wanted.retain(|id, need| {
            *need != Need::InView || self.get(*id).is_some_and(|entity| self.needed_now(entity))
        });
        let Some(most) = wanted.values().max().copied() else {
            return Vec::new();
        };
        let (files, bytes) = READ;
        let mut taken = Vec::new();
        let mut held = 0;
        for (id, _) in wanted.iter().filter(|(_, need)| **need == most) {
            if taken.len() == files || held >= bytes {
                break;
            }
            held += self.get(*id).map_or(0, LocalEntity::size);
            taken.push(*id);
        }
        for id in &taken {
            wanted.remove(id);
        }
        drop(wanted);
        self.asked.extend(taken.iter().copied());
        taken
    }

    /// Whether `id` is wanted or asked of the library and not yet answered.
    #[cfg(test)]
    pub fn wanted(&self, id: u64) -> bool {
        self.wanted.borrow().contains_key(&id) || self.asked.contains(&id)
    }

    /// Whether any read asked of the library is still to be answered.
    #[cfg(test)]
    pub fn asking(&self) -> bool {
        !self.asked.is_empty()
    }

    /// The library's answer to an asset asked for: its bytes, decoded off the frame, ahead
    /// of anything else waiting where something needs it now, or the file it rests in, or
    /// nothing, when the library no longer has its file and it stays unread. They are
    /// what it was saved as, since nothing edits an asset not read.
    pub fn took(&mut self, id: u64, bytes: Option<Vec<u8>>, file: Option<Arc<OnDisk>>) {
        self.asked.remove(&id);
        let Some(entity) = self.get(id).filter(|entity| entity.unread()) else {
            return;
        };
        let (stamp, needed) = (entity.stamp, self.needed_now(entity));
        match (bytes, file) {
            (Some(bytes), _) => {
                self.swap(id, false, |held| {
                    let (name, origin) = (held.name.clone(), held.origin.clone());
                    LocalEntity::undecoded(id, name, origin, bytes.into(), stamp)
                });
                self.undecoded.push_back(id);
                if needed {
                    self.hurried.get_mut().insert(id);
                }
            }
            (None, Some(file)) => self.rest(id, file, stamp),
            (None, None) => {}
        }
    }

    /// The library could not read an asset asked for, and why. It stays unread, and is
    /// not asked for again.
    pub fn unreadable(&mut self, id: u64, why: String, log: &mut Log) {
        self.asked.remove(&id);
        let Some(entity) = self.get_mut(id).filter(|entity| entity.unread()) else {
            return;
        };
        log.warn(format!("{}: {why}", entity.name));
        entity.parse_error = Some(why.clone());
        entity.verify = VerifyState::NotRead(why);
        entity.remembered = None;
        self.revision += 1;
    }

    /// Whether something needs an unread asset not yet asked of the library.
    pub fn wants(&self) -> bool {
        !self.wanted.borrow().is_empty()
    }

    /// The listed length of every file asked of the library and not yet answered.
    pub fn asked_whole(&self) -> u64 {
        let asked = self.asked.iter().filter_map(|id| self.get(*id));
        asked
            .filter(|entity| entity.unread())
            .map(LocalEntity::size)
            .sum()
    }

    /// Ask the library again for an asset it could not read, as something needs it.
    pub fn retry(&mut self, id: u64) {
        let Some(entity) = self.get_mut(id).filter(|entity| entity.unread()) else {
            return;
        };
        if !matches!(entity.verify, VerifyState::NotRead(_)) {
            return;
        }
        entity.parse_error = None;
        entity.verify = VerifyState::Reading;
        self.revision += 1;
        self.wanted.get_mut().insert(id, Need::Now);
    }

    /// Ask the library again for an asset whose read was refused for want of room, now
    /// that room has been made for it.
    pub fn again(&mut self, id: u64) {
        self.asked.remove(&id);
        if self.get(id).is_some_and(LocalEntity::unread) {
            self.wanted.get_mut().insert(id, Need::Now);
            self.ctx.request_repaint();
        }
    }

    /// Count an unread asset as asked of the library for a read in the background, which
    /// nothing waits on. Returns `false`, and counts nothing, where it is not unread or is
    /// wanted or asked already.
    pub fn ask_behind(&mut self, id: u64) -> bool {
        let unread = self.get(id).is_some_and(LocalEntity::unread);
        if !unread || self.wanted.get_mut().contains_key(&id) {
            return false;
        }
        self.asked.insert(id)
    }

    /// The library refused to read an asset asked of it, for want of room. It stays as it
    /// was until it is asked again.
    pub fn unasked(&mut self, id: u64) {
        self.asked.remove(&id);
    }

    /// Whether something needed this asset this frame or the last. See
    /// [`Workspace::hurry`].
    fn needed_now(&self, entity: &LocalEntity) -> bool {
        let seen = entity.seen.get();
        seen > 0 && seen + 1 >= self.frame
    }

    /// The frame something last needed this asset in, 0 for never, and how many bytes
    /// letting it go would free. `None` where it must be kept whole: it is a view, unsaved
    /// or holding an editor's pending edit, unread or resting in its file, or needed now.
    pub fn evictable(&self, id: u64) -> Option<(u64, u64)> {
        let entity = self.get(id)?;
        let held = entity.held_whole();
        let kept = !entity.kept
            || entity.is_unsaved()
            || entity.unread()
            || entity.rests().is_some()
            || self.needed_now(entity);
        (!kept && held > 0).then_some((entity.seen.get(), held))
    }

    /// Let go of what a clean asset holds whole and what it decodes to, so it is unread
    /// again under a new stamp, which this returns. It keeps its summary, so it still
    /// draws as it did and matches its slot. `None`, and nothing let go, where
    /// [`Workspace::evictable`] keeps it.
    pub fn evict(&mut self, id: u64) -> Option<u64> {
        self.evictable(id)?;
        let entity = self.get(id)?;
        let (len, crc32, known) = (entity.size(), entity.saved.crc32, Summary::of(entity));
        let stamp = self.stamp();
        self.swap(id, false, |held| {
            let (name, origin) = (held.name.clone(), held.origin.clone());
            let mut listed = LocalEntity::listed(id, name, origin, len, stamp);
            listed.saved.crc32 = crc32;
            if let Some(known) = known {
                listed.know(known);
            }
            listed
        });
        Some(stamp)
    }

    /// Give an unread asset what a read of its file found before, so it draws as it did
    /// once read without being read. Nothing for an asset read since, or one that could
    /// not be read. A read wanted only for its row is no longer asked for; something
    /// that needs it whole asks again.
    pub fn remember(&mut self, id: u64, summary: Summary) {
        let Some(entity) = self.get_mut(id) else {
            return;
        };
        if entity.unread() && entity.reading() {
            entity.know(summary);
            self.wanted.get_mut().remove(&id);
            self.revision += 1;
        }
    }

    /// An unread asset's file changed on disk: what it remembers of it from before no
    /// longer stands for it.
    pub fn stale(&mut self, id: u64) {
        let Some(entity) = self.get_mut(id).filter(|entity| entity.unread()) else {
            return;
        };
        let crc32 = entity.saved.crc32.take();
        let known = entity.remembered.take();
        if matches!(entity.verify, VerifyState::Remembered(_)) {
            entity.verify = VerifyState::Reading;
        }
        if crc32.is_some() || known.is_some() {
            self.revision += 1;
        }
    }

    /// The projects listed here that name the WAV at `wav`: each one held, by what it
    /// holds now, and each unread by what a read of it found before.
    pub fn projects_naming(&self, wav: &LibPath) -> Naming {
        let mut naming = Naming::default();
        let projects = self
            .listed()
            .filter(|entity| crate::browser::Kind::of(entity) == crate::browser::Kind::Project);
        for entity in projects {
            let Some(dir) = entity.path.as_ref().map(LibPath::parent) else {
                continue;
            };
            let Some(known) = Summary::of(entity) else {
                naming.unknown.push(entity.id);
                continue;
            };
            let resolved = known
                .wavs
                .iter()
                .map(|named| crate::summary::resolve(&dir, named));
            if resolved.flatten().any(|named| named == *wav) {
                naming.by.push(entity.id);
            }
        }
        naming
    }

    /// How many bytes the assets hold whole in memory, their baselines' included.
    pub fn held_whole(&self) -> u64 {
        self.entities.iter().map(LocalEntity::held_whole).sum()
    }

    /// Decode these assets now, on this thread, where they are read and still to be
    /// decoded: something is about to act on what they decode to.
    pub fn read_now(&mut self, ids: impl IntoIterator<Item = u64>, log: &mut Log) {
        for id in ids {
            let Some(entity) = self.get(id) else {
                continue;
            };
            entity.seen.set(self.frame);
            if !entity.reading() || entity.unread() {
                continue;
            }
            let (stamp, decoded) = (entity.stamp, Decoded::of(&entity.bytes));
            self.take_decode(id, stamp, decoded, log);
        }
    }

    /// Fold in the decodes that have answered, and start the next.
    fn decode(&mut self, log: &mut Log) {
        for Decode { ids, job } in std::mem::take(&mut self.decoding) {
            match job.poll() {
                work::Answer::Running => self.decoding.push(Decode { ids, job }),
                answer => self.decodes(ids, answer, log),
            }
        }
        self.next_decodes();
        #[cfg(target_arch = "wasm32")]
        if !self.decoding.is_empty() || !self.undecoded.is_empty() {
            self.ctx.request_repaint();
        }
    }

    fn decodes(
        &mut self,
        ids: Vec<u64>,
        answer: work::Answer<Vec<(u64, u64, Decoded)>>,
        log: &mut Log,
    ) {
        for id in &ids {
            self.flying.remove(id);
        }
        let decoded = match answer {
            work::Answer::Running => return,
            work::Answer::Answered(decoded) => decoded,
            work::Answer::Died => {
                let why = "the decode stopped without an answer";
                ids.into_iter()
                    .filter_map(|id| Some((id, self.get(id)?.stamp, Decoded::failed(why))))
                    .collect()
            }
        };
        for (id, stamp, decoded) in decoded {
            self.take_decode(id, stamp, decoded, log);
        }
    }

    /// Give an asset what its bytes decode to, unless it no longer holds those bytes or
    /// has its decode already.
    fn take_decode(&mut self, id: u64, stamp: u64, decoded: Decoded, log: &mut Log) {
        let Some(entity) = self.get_mut(id) else {
            return;
        };
        if !entity.reading() || entity.stamp != stamp {
            return;
        }
        entity.decoded(decoded);
        // A note has no format to decode, so a parse error on text is not a failure.
        if let Some(e) = entity.parse_error.as_ref().filter(|_| !entity.is_text) {
            log.warn(format!("{}: {e}", entity.name));
        }
        self.revision += 1;
    }

    /// Start decodes, up to [`DECODES`] at once, of the assets asked for first and then
    /// of the rest in the order they arrived.
    fn next_decodes(&mut self) {
        while self.decoding.len() < DECODES {
            let (most, most_bytes) = DECODE;
            let mut chunk = Vec::new();
            let mut bytes = 0;
            while chunk.len() < most && bytes < most_bytes {
                let next = self.hurried.get_mut().pop_first();
                let Some(id) = next.or_else(|| self.undecoded.pop_front()) else {
                    break;
                };
                if self.flying.contains(&id) {
                    continue;
                }
                let Some(entity) = self.get(id).filter(|entity| entity.reading()) else {
                    continue;
                };
                if entity.unread() {
                    continue;
                }
                bytes += entity.bytes.len() as u64;
                chunk.push((id, entity.stamp, entity.bytes.clone()));
            }
            if chunk.is_empty() {
                return;
            }
            let ids: Vec<u64> = chunk.iter().map(|(id, _, _)| *id).collect();
            self.flying.extend(ids.iter().copied());
            let job = work::run(&self.ctx, move |_| {
                chunk
                    .into_iter()
                    .map(|(id, stamp, bytes)| (id, stamp, Decoded::of(&bytes)))
                    .collect()
            });
            self.decoding.push(Decode { ids, job });
        }
    }

    /// Decode `bytes`, verify them, and add the row, logging the details of what
    /// arrived. Every way in (drop, picker, fresh default, device read) lands here.
    ///
    /// The caller writes the status line: [`Workspace::ingest`] announces something on
    /// this computer, and [`Workspace::view`] a slot being viewed.
    fn add(
        &mut self,
        name: String,
        origin: Origin,
        bytes: Vec<u8>,
        log: &mut Log,
    ) -> (u64, Arrival) {
        let id = self.next_id;
        self.next_id += 1;
        let entity = LocalEntity::new(id, name, origin, bytes.into(), self.stamp());
        let arrival = match (&entity.parse_error, &entity.verify) {
            // A note has no format to decode, so a parse error on text is not a
            // failure.
            (Some(_), _) if entity.is_text => {
                log.info(format!(
                    "{}: text ({} bytes)",
                    entity.name,
                    entity.bytes.len()
                ));
                Arrival::Read
            }
            (Some(e), _) => {
                log.error(format!("{}: {e}", entity.name));
                Arrival::Unreadable
            }
            (None, VerifyState::Ok) => {
                log.info(format!(
                    "{}: {} ({} bytes), verified",
                    entity.name,
                    entity.tag(),
                    entity.bytes.len(),
                ));
                Arrival::Read
            }
            (None, state) => {
                log.warn(format!(
                    "{}: {} — verify {}: {}",
                    entity.name,
                    entity.tag(),
                    state.badge(),
                    state.detail(),
                ));
                Arrival::Unverified
            }
        };
        self.entities.push(entity);
        self.layout += 1;
        (id, arrival)
    }

    /// Take a copy of `from`, about `len` bytes, onto this computer at `path`: an asset
    /// unread until the library has copied the file there, which it then reads as any
    /// file of its own.
    pub fn arrive(&mut self, path: LibPath, origin: Origin, from: CopyOf, len: u64) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let name = path.leaf().to_string();
        let stamp = self.stamp();
        self.entities.push(LocalEntity {
            path: Some(path),
            ..LocalEntity::listed(id, name, origin, len, stamp)
        });
        self.layout += 1;
        self.arriving.insert(id, from);
        id
    }

    /// Copy `from`, about `len` bytes, over the file of the asset `id`, which is unread
    /// until the copy lands. Its tags and its place stay.
    pub fn arrive_over(&mut self, id: u64, from: CopyOf, len: u64) {
        self.relist(id, len);
        if self.get(id).is_some() {
            self.arriving.insert(id, from);
        }
    }

    /// Copy the file the asset `from` rests in, with any edit held of it, over the file
    /// of the asset `id`, and let `from` go once the copy has landed: see
    /// [`Workspace::moved_over`]. Returns `false`, and copies nothing, where `from` rests
    /// in no file a copy can be made of.
    pub fn move_over(&mut self, id: u64, from: u64) -> bool {
        let (Some(copy), Some(source)) = (self.copy_of(from), self.get(from)) else {
            return false;
        };
        let len = source.size();
        let leaving = self.leaving(from);
        self.arrive_over(id, copy, len);
        self.moving_over
            .extend(leaving.map(|leaving| (id, leaving)));
        true
    }

    /// The asset a copy that has answered over the file of `id` was moved from, which
    /// goes where the copy landed and stays where it did not.
    pub fn moved_over(&mut self, id: u64) -> Option<Leaving> {
        self.moving_over.remove(&id)
    }

    /// `from` as it is now, about to be copied or saved over another asset.
    fn leaving(&self, from: u64) -> Option<Leaving> {
        Some(Leaving {
            from,
            edit: self.edited_at(from)?,
        })
    }

    /// The stamp of what an asset holds: of the edit held of it, or of its bytes.
    fn edited_at(&self, id: u64) -> Option<u64> {
        let entity = self.get(id)?;
        Some(self.kept_edit(id).map_or(entity.stamp, |(_, stamp)| stamp))
    }

    /// Whether the asset `leaving` came from still holds what was copied or saved of it.
    pub fn unchanged(&self, leaving: &Leaving) -> bool {
        self.edited_at(leaving.from) == Some(leaving.edit)
    }

    /// Put `bytes` over `id` as its next save. Bytes that came from the asset `from` leave
    /// it until that save has landed ([`Workspace::saved_over`]), and are returned to be
    /// removed now only where `id` already held them and needs no save.
    pub fn save_over(
        &mut self,
        id: u64,
        bytes: Vec<u8>,
        from: Option<u64>,
        log: &mut Log,
    ) -> Option<u64> {
        self.replace_bytes(id, bytes, log);
        let unsaved = self.get(id).filter(|entity| entity.is_unsaved());
        let Some(stamp) = unsaved.map(|entity| entity.stamp) else {
            return from;
        };
        self.mark_saved(id);
        if let Some(leaving) = from.and_then(|from| self.leaving(from)) {
            self.saving_over.insert(id, (leaving, stamp));
        }
        None
    }

    /// The asset an overwrite of `id` came from, once a save of `id` has answered: a save
    /// of the stamp the overwrite wrote, or a later one, landed with `landed`, or failed
    /// without. `None` while the overwrite's save is still to answer.
    pub fn saved_over(&mut self, id: u64, saved: u64, landed: bool) -> Option<Leaving> {
        let (_, stamp) = self.saving_over.get(&id)?;
        if saved < *stamp {
            return None;
        }
        let (leaving, _) = self.saving_over.remove(&id)?;
        landed.then_some(leaving)
    }

    /// Copy `from` in again for an asset whose copy did not land.
    pub fn arrive_again(&mut self, id: u64, from: CopyOf) {
        if self.get(id).is_some() {
            self.arriving.insert(id, from);
        }
    }

    /// What an asset's copy is a copy of, while it is arriving.
    pub fn arriving(&self, id: u64) -> Option<&CopyOf> {
        self.arriving.get(&id)
    }

    /// The arriving assets whose copies are of the file `id` rests in.
    pub fn copies_of(&self, id: u64) -> impl Iterator<Item = u64> + '_ {
        let copies = self.arriving.iter();
        copies
            .filter(move |(_, from)| matches!(from, CopyOf::Asset(source) if *source == id))
            .map(|(copy, _)| *copy)
    }

    /// An asset's copy has answered: it no longer arrives.
    pub fn arrived(&mut self, id: u64) -> Option<CopyOf> {
        self.arriving.remove(&id)
    }

    /// What a copy of an asset resting in its file is a copy of: the file with the edit
    /// held of it written through, or the file as it is. `None` for one that does not
    /// rest in a file, or holds an edit that does not apply to it.
    pub fn copy_of(&self, id: u64) -> Option<CopyOf> {
        self.get(id)?.rests()?;
        let Some(held) = self.edits.get(&id) else {
            return Some(CopyOf::Asset(id));
        };
        let (over, rewrite) = (held.over.as_ref()?, held.rewrite.as_ref().ok()?);
        Some(CopyOf::Edited(over.clone(), rewrite.clone()))
    }

    /// Let go of what an asset holds, unread again as a file of `len` bytes under a new
    /// stamp, for a file that holds something else now.
    pub fn relist(&mut self, id: u64, len: u64) {
        let stamp = self.stamp();
        self.swap(id, false, |held| {
            let (name, origin) = (held.name.clone(), held.origin.clone());
            LocalEntity::listed(id, name, origin, len, stamp)
        });
    }

    /// Read the file outside the library at `from` whole, and open it as a file kept
    /// nowhere yet, as a library that cannot take a copy of it holds it.
    pub fn read_outside(&self, name: String, from: Outside) {
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        spawn(async move {
            let _ = tx.send(match read_outside(&from).await {
                Ok(bytes) => Incoming::Opened { name, bytes },
                Err(e) => Incoming::Failed(format!("{name}: {e}")),
            });
            ctx.request_repaint();
        });
    }

    /// Take `bytes` onto this computer, and say so.
    pub fn ingest(&mut self, name: String, origin: Origin, bytes: Vec<u8>, log: &mut Log) -> u64 {
        let (id, arrival) = self.add(name.clone(), origin, bytes, log);
        match arrival {
            Arrival::Unreadable => {
                log.trouble(format!("“{name}” is not a file this app understands."))
            }
            Arrival::Read => log.say(format!("“{name}” is on this computer.")),
            Arrival::Unverified => log.say(format!(
                "“{name}” opened, but it does not re-save byte for byte."
            )),
        }
        id
    }

    /// Bump the revision and return it as the stamp for a new set of bytes.
    fn stamp(&mut self) -> u64 {
        self.revision += 1;
        self.revision
    }

    /// The stamp a baseline of `bytes` takes under `id`: the asset's current stamp if it
    /// holds those same bytes, and a new stamp otherwise.
    ///
    /// ⚠️ Compares two bodies, so it runs only when a baseline moves and never per frame.
    /// That keeps [`LocalEntity::is_unsaved`] and the caches over the pair a comparison
    /// of two integers.
    fn stamp_for(&mut self, id: u64, bytes: &[u8]) -> u64 {
        match self
            .get(id)
            .map(|entity| (entity.stamp, entity.holds(bytes)))
        {
            Some((stamp, true)) => stamp,
            _ => self.stamp(),
        }
    }

    /// Drain whatever the pickers finished with, fold in the checks, reads and decodes
    /// that have answered and start the next, and return the files picked to open, by
    /// name. Call once per frame.
    pub fn poll(&mut self, log: &mut Log) -> Vec<(String, Vec<u8>)> {
        self.frame += 1;
        let mut opened = Vec::new();
        while let Ok(message) = self.rx.try_recv() {
            match message {
                Incoming::Opened { name, bytes } => opened.push((name, bytes)),
                Incoming::Picked(from) => self.picked.push(from),
                Incoming::Wavs { making, files } => self.draft = Draft::plan(making, files),
                Incoming::Demos(files) => self.demos = Some(files),
                Incoming::Note(text) => log.say(text),
                Incoming::Failed(text) => log.trouble(text),
            }
        }
        if let Some(check) = &self.checking {
            let answer = check.job.poll();
            self.checked(answer, log);
        }
        self.decode(log);
        opened
    }

    /// Wait for every check, read and decode in flight, and fold them in.
    #[cfg(test)]
    pub fn settle_files(&mut self, log: &mut Log) {
        loop {
            self.next_decodes();
            let Some(Decode { ids, job }) = self.decoding.pop() else {
                break;
            };
            let answer = job.wait();
            self.decodes(ids, answer, log);
        }
        while let Some(check) = &self.checking {
            let answer = check.job.wait();
            self.checked(answer, log);
        }
    }

    pub fn open_dialog(&self) {
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        spawn(async move {
            let picked = rfd::AsyncFileDialog::new()
                .set_title("Open Nord files")
                .pick_files()
                .await;
            for handle in picked.unwrap_or_default() {
                let _ = tx.send(Incoming::Picked(handle.inner().to_owned()));
            }
            ctx.request_repaint();
        });
    }

    /// Fetch the demo sounds, to be filed once they arrive; see [`Workspace::take_demos`].
    pub fn fetch_demos(&self) {
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        spawn(async move {
            let _ = tx.send(match crate::demo::fetch().await {
                Ok(files) => Incoming::Demos(files),
                Err(why) => Incoming::Failed(why),
            });
            ctx.request_repaint();
        });
    }

    /// The demo files a fetch brought back, once nothing that might already hold one is
    /// still to be read. Until then each such asset is asked of the library.
    pub fn take_demos(&mut self) -> Option<Vec<(String, Vec<u8>)>> {
        let unsure = crate::demo::unsure(self.demos.as_ref()?, self);
        for id in &unsure {
            self.hurry(*id);
        }
        unsure.is_empty().then(|| self.demos.take()).flatten()
    }

    /// Whether an asset on this computer holds `bytes`, or was last saved as them. An
    /// asset not read yet is not known to.
    pub fn holds(&self, bytes: &[u8]) -> bool {
        self.listed()
            .any(|e| (!e.unread() && *e.bytes == *bytes) || e.saved.holds(bytes))
    }

    /// The files File ▸ Open… picked since the last call.
    pub fn take_picked(&mut self) -> Vec<Outside> {
        std::mem::take(&mut self.picked)
    }

    /// Pick the WAVs a new project or instrument is laid out from.
    pub fn pick_wavs(&self, making: Making) {
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        spawn(async move {
            let picked = rfd::AsyncFileDialog::new()
                .set_title(format!("Pick the WAVs for a {}", making.label()))
                .add_filter("WAV", &["wav"])
                .pick_files()
                .await;
            let mut files = Vec::new();
            for handle in picked.unwrap_or_default() {
                let bytes = handle.read().await;
                files.push((handle.file_name(), bytes));
            }
            let _ = tx.send(Incoming::Wavs { making, files });
            ctx.request_repaint();
        });
    }

    /// The picked WAVs waiting on their root keys, for the dialog to edit.
    pub fn draft_mut(&mut self) -> Option<&mut Draft> {
        self.draft.as_mut()
    }

    /// Take the draft away, whether it is about to be made or abandoned.
    pub fn take_draft(&mut self) -> Option<Draft> {
        self.draft.take()
    }

    /// The filename an export suggests.
    ///
    /// ⚠️ Only filenames are made path-safe. Everywhere else the name is kept as the
    /// instrument or the user wrote it, spaces and all, and a rename sends that spelling
    /// back to the instrument.
    pub fn export_name(&self, id: u64) -> Option<String> {
        let entity = self.get(id)?;
        Some(match entity.rests() {
            Some(file) => tagged_filename(&entity.name, || file.index.tag().to_string()),
            None => export_filename(&entity.name, &entity.bytes),
        })
    }

    pub fn export(&self, id: u64) {
        let Some(entity) = self.get(id) else {
            return;
        };
        let name = match self.export_name(id) {
            Some(name) => name,
            None => return,
        };
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(file) = entity.rests() {
            let (tx, ctx, file) = (self.tx.clone(), self.ctx.clone(), file.clone());
            spawn(async move {
                let _ = tx.send(save_file(name, file).await);
                ctx.request_repaint();
            });
            return;
        }
        #[cfg(target_arch = "wasm32")]
        if let Some(file) = entity.rests() {
            let _ = self.tx.send(save_file(&name, file));
            self.ctx.request_repaint();
            return;
        }
        self.save_bytes(name, entity.bytes.to_vec());
    }

    /// Hand bytes to the user under `name`, through this target's way of saving a file.
    /// Exports of whole assets and of per-zone WAVs both use it.
    pub fn save_bytes(&self, name: String, bytes: Vec<u8>) {
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        spawn(async move {
            let _ = tx.send(save(name, bytes).await);
            ctx.request_repaint();
        });
    }

    /// Restore the bytes this asset was last saved as, and drop any pending edit an
    /// editor held of it.
    ///
    /// ⚠️ A pending edit has not reached the bytes, so restoring them changes nothing.
    /// Reverting a piano library's plan only clears the flag.
    pub fn revert(&mut self, id: u64, log: &mut Log) {
        let Some(entity) = self.get(id) else {
            return;
        };
        let (resting, file, stamp) = (
            entity.rests().is_some(),
            entity.saved.file.clone(),
            entity.saved.stamp,
        );
        let in_memory = !resting && file.is_none() && entity.saved.unread.is_none();
        let saved = in_memory.then(|| entity.saved.bytes.clone());
        let let_go = self.let_go_edit(id);
        let dropped = self.mark_pending(id, false) || let_go;
        let restored = match (resting, file, saved) {
            (false, Some(file), _) => {
                self.rest(id, file, stamp);
                true
            }
            (false, None, Some(saved)) => self.respell(id, saved).is_some(),
            _ => false,
        };
        if !restored && !dropped {
            return;
        }
        if let Some(entity) = self.get(id) {
            log.say(format!("“{}” is back as it was last saved.", entity.name));
        }
    }

    /// Record whether an editor holds an edit of this asset that its bytes do not, and
    /// return whether that changed. See [`LocalEntity::is_unsaved`]. An asset holding an
    /// edit over the file it rests in counts as holding one whatever the editor says.
    ///
    /// ⚠️ Only the editor holding the edit can tell, so it must call this on every frame
    /// the answer might change. The bytes are the saved ones either way.
    pub fn mark_pending(&mut self, id: u64, pending: bool) -> bool {
        let pending = pending || self.edits.contains_key(&id);
        let Some(entity) = self.get_mut(id) else {
            return false;
        };
        if std::mem::replace(&mut entity.pending, pending) == pending {
            return false;
        }
        self.revision += 1;
        true
    }

    /// Hold the edit an editor made of an asset resting in its file, made over that file,
    /// or let it go with `None`. An edit that changes nothing there is let go.
    ///
    /// ⚠️ A save already sent carries on with the edit it was sent with, and an edit
    /// made since is made again over the file it writes.
    pub fn hold_edit(&mut self, id: u64, edit: Option<Edit>) {
        let save = self.edits.get(&id).map_or(Save::No, |held| held.save);
        let Some(edit) = edit else {
            if !matches!(save, Save::Sent(_)) {
                self.let_go_edit(id);
            }
            return;
        };
        let Some(over) = self.get(id).and_then(LocalEntity::rests).cloned() else {
            return;
        };
        let stamp = self.stamp();
        self.make_edit(id, edit, stamp, Some(over), save);
    }

    /// Hold an edit kept across a quit of an asset just restored, made over the file it
    /// rests in. One that file already holds is let go.
    pub fn restore_edit(&mut self, id: u64, edit: Edit) {
        let Some(entity) = self.get(id) else {
            return;
        };
        let over = entity.rests().cloned();
        let stamp = self.stamp();
        self.make_edit(id, edit, stamp, over, Save::No);
    }

    /// Hold `edit` of an asset made over `over`, the file it rests in, or let it go where
    /// it changes nothing there.
    fn make_edit(
        &mut self,
        id: u64,
        edit: Edit,
        stamp: u64,
        over: Option<Arc<OnDisk>>,
        save: Save,
    ) {
        let made = match &over {
            Some(file) => edit.over(file),
            None => Err(
                "the file is not a piano library or sample instrument resting in \
                         the library"
                    .to_string(),
            ),
        };
        let rewrite = match made {
            Ok(None) => {
                self.let_go_edit(id);
                return;
            }
            Ok(Some(rewrite)) => Ok(Arc::new(rewrite)),
            Err(why) => Err(why),
        };
        let held = Held {
            edit,
            stamp,
            over,
            rewrite,
            save,
        };
        self.edits.insert(id, held);
        self.mark_pending(id, true);
        self.revision += 1;
    }

    /// Make the edit held of an asset again over the file it rests in, where that is
    /// another file than the one it was made over. A save asked of it is asked again
    /// only once someone asks, since the file it would write over is not the one it was
    /// asked of.
    fn follow_edit(&mut self, id: u64) {
        let Some(file) = self.get(id).and_then(LocalEntity::rests).cloned() else {
            return;
        };
        let Some(held) = self.edits.get(&id) else {
            return;
        };
        let over = held.over.as_ref();
        if over.is_some_and(|over| Arc::ptr_eq(over, &file)) || matches!(held.save, Save::Sent(_)) {
            return;
        }
        let (edit, stamp) = (held.edit.clone(), held.stamp);
        self.make_edit(id, edit, stamp, Some(file), Save::No);
    }

    /// Let go of the edit held of an asset, and return whether there was one.
    fn let_go_edit(&mut self, id: u64) -> bool {
        if self.edits.remove(&id).is_none() {
            return false;
        }
        self.mark_pending(id, false);
        self.revision += 1;
        true
    }

    /// The edit an editor made of an asset resting in its file, whether or not it applies
    /// to the file it rests in now.
    pub fn edit(&self, id: u64) -> Option<&Edit> {
        self.edits.get(&id).map(|held| &held.edit)
    }

    /// Why the edit held of an asset does not apply to the file it rests in, where it
    /// does not.
    pub fn unapplied(&self, id: u64) -> Option<&str> {
        self.edits
            .get(&id)?
            .rewrite
            .as_ref()
            .err()
            .map(String::as_str)
    }

    /// The assets an edit is held of.
    pub fn edited(&self) -> impl Iterator<Item = u64> + '_ {
        self.edits.keys().copied()
    }

    /// The edit held of an asset, whether or not it applies, and its stamp, which moves
    /// whenever the edit does: what a working copy keeps of it.
    pub(crate) fn kept_edit(&self, id: u64) -> Option<(&Edit, u64)> {
        self.edits.get(&id).map(|held| (&held.edit, held.stamp))
    }

    /// The edit held of an asset resting in its file, where it applies to that file: the
    /// file it is made over, and the rewrite that writes it.
    pub fn edit_of(&self, id: u64) -> Option<(&Arc<OnDisk>, &Arc<Rewrite>)> {
        let held = self.edits.get(&id)?;
        Some((held.over.as_ref()?, held.rewrite.as_ref().ok()?))
    }

    /// Ask the store to save the edit held of an asset resting in its file into that
    /// file. Returns whether there is an edit that applies to save.
    pub fn save_edit(&mut self, id: u64) -> bool {
        let Some(held) = self.edits.get_mut(&id).filter(|held| held.rewrite.is_ok()) else {
            return false;
        };
        if held.save == Save::No {
            held.save = Save::Asked;
            self.revision += 1;
        }
        true
    }

    /// Whether a save of an asset's edit is asked for and not yet answered.
    pub fn saving_edit(&self, id: u64) -> bool {
        self.edits
            .get(&id)
            .is_some_and(|held| held.save != Save::No)
    }

    /// The assets whose edits the store is asked to save and has not sent.
    pub(crate) fn edits_to_save(&self) -> Vec<u64> {
        let asked = self
            .edits
            .iter()
            .filter(|(_, held)| held.save == Save::Asked);
        asked.map(|(id, _)| *id).collect()
    }

    /// Take an edit the store is asked to save, as it sends the save.
    pub(crate) fn send_edit(&mut self, id: u64) -> Option<(Arc<OnDisk>, Arc<Rewrite>)> {
        let held = self.edits.get_mut(&id)?;
        let (over, rewrite) = (held.over.clone()?, held.rewrite.as_ref().ok()?.clone());
        held.save = Save::Sent(held.stamp);
        Some((over, rewrite))
    }

    /// An edit's save did not land. The edit stays, made over the file the asset rests in
    /// now, and is saved again only when asked.
    pub fn edit_not_saved(&mut self, id: u64) {
        if let Some(held) = self.edits.get_mut(&id) {
            held.save = Save::No;
            self.revision += 1;
        }
        self.follow_edit(id);
    }

    /// An edit's save landed in `file`, which the asset now rests in. An edit made since
    /// the save was sent stays, made again over that file.
    pub fn edit_saved(&mut self, id: u64, file: Arc<OnDisk>) {
        let sent = self.edits.get_mut(&id).map(|held| {
            let sent = held.save == Save::Sent(held.stamp);
            held.save = Save::No;
            sent
        });
        if sent == Some(true) {
            self.let_go_edit(id);
        }
        self.adopt_file(id, file);
    }

    /// Make the current bytes the saved baseline.
    ///
    /// ⚠️ The bytes do not change, so the stamp stays and caches over them stay valid.
    /// The list revision moves, so the store is written again without the unsaved copy.
    pub fn mark_saved(&mut self, id: u64) {
        let Some(entity) = self.get_mut(id) else {
            return;
        };
        // Resting in its file, it holds what it was saved as, and only an editor's pending
        // edit, which the bytes do not hold, can make it unsaved.
        if !entity.is_unsaved() || entity.rests().is_some() {
            return;
        }
        entity.saved = entity.baseline();
        self.revision += 1;
    }

    /// Record a write that reached a slot: the bytes it carried become the saved
    /// baseline, and the slot becomes the link.
    ///
    /// ⚠️ The baseline is the bytes the send carried, never the bytes held now. A write
    /// takes as long as the instrument takes, and an edit made while one was in flight
    /// exists only on this computer. Calling it saved would let it be discarded with its
    /// tab.
    ///
    /// ⚠️ The only place a link is set instead of derived. A write is the only evidence
    /// about a slot this app does not have to read back, and [`crate::device::link`]
    /// keeps it until a walk of that slot says otherwise.
    pub fn landed(&mut self, id: u64, class: ObjectClass, at: Location, sent: Vec<u8>) {
        let resting = self
            .get(id)
            .filter(|entity| entity.rests().is_some())
            .map(|entity| entity.saved.reports(&sent));
        match resting {
            Some(true) => {
                if let Some(entity) = self.get_mut(id) {
                    entity.link = Some((class, at));
                    entity.wrote = entity.saved.crc32.map(|crc32| Wrote { class, at, crc32 });
                }
                self.revision += 1;
                return;
            }
            // The file changed under the send, and what the instrument now holds is what
            // was sent.
            Some(false) => {
                self.respell(id, sent.clone().into());
            }
            None => {}
        }
        let stamp = self.stamp_for(id, &sent);
        let Some(entity) = self.get_mut(id) else {
            return;
        };
        let sent = match stamp == entity.stamp {
            true => entity.bytes.clone(),
            false => sent.into(),
        };
        entity.saved = Baseline::read(sent, stamp);
        entity.link = Some((class, at));
        entity.wrote = entity.saved.crc32.map(|crc32| Wrote { class, at, crc32 });
        self.revision += 1;
    }

    /// Record a write that sent the file `file` to a slot, read from it a chunk at a time:
    /// the slot becomes the link, and the write is recorded with `crc32`, the checksum a
    /// slot holding the file reports.
    ///
    /// ⚠️ An asset resting in another file now, as one whose edit was saved while the
    /// file was sent does, keeps resting there: the edit is on disk, and the slot holds
    /// what it was saved as before. One holding bytes in memory instead is saved as the
    /// file again, under a stamp of its own, for the reason [`Workspace::landed`] gives.
    pub fn landed_file(
        &mut self,
        id: u64,
        class: ObjectClass,
        at: Location,
        file: Arc<OnDisk>,
        crc32: u32,
    ) {
        let moved = self.get(id).is_some_and(|entity| {
            let saved = &entity.saved;
            let other = saved.size() != file.len || saved.crc32 != Some(crc32);
            entity.rests().is_none() && other
        });
        let stamp = match moved {
            true => Some(self.stamp()),
            false => None,
        };
        let Some(entity) = self.get_mut(id) else {
            return;
        };
        if let Some(stamp) = stamp {
            entity.saved = Baseline {
                crc32: Some(crc32),
                ..Baseline::on_disk(file, stamp)
            };
        }
        entity.link = Some((class, at));
        entity.wrote = Some(Wrote { class, at, crc32 });
        self.revision += 1;
    }

    /// Forget every write this app made.
    ///
    /// ⚠️ A write is evidence about the instrument that took it. With none attached, or
    /// another one in its place, it says nothing about what is in any slot.
    pub fn forget_writes(&mut self) {
        for entity in &mut self.entities {
            entity.wrote = None;
        }
    }

    /// Swap in edited bytes, keeping the entity's identity.
    ///
    /// The decode and the verify run again: an editor's output is bytes like any other and
    /// is checked the same way as a file from disk.
    pub fn replace_bytes(&mut self, id: u64, bytes: Vec<u8>, log: &mut Log) {
        let Some(verify) = self.respell(id, bytes.into()) else {
            return;
        };
        let note = self.get(id).is_some_and(|held| held.is_text);
        if note || matches!(verify, VerifyState::Ok) {
            return;
        }
        log.warn(format!(
            "after editing, verify {}: {}",
            verify.badge(),
            verify.detail()
        ));
    }

    /// Put different bytes under one id, rebuild everything derived from them, and return
    /// the verify result.
    ///
    /// ⚠️ The saved baseline is kept: it moves only when the asset is saved, so an edit
    /// and its revert are measured against the same bytes. The link and the last write
    /// are kept too, because both are evidence about a slot, which an edit here says
    /// nothing about.
    fn respell(&mut self, id: u64, bytes: Bytes) -> Option<VerifyState> {
        if self.get(id).is_none_or(|entity| entity.holds(&bytes)) {
            return None;
        }
        // The baseline of an asset still being decoded stays, and is read here. One whose
        // file was never read keeps a baseline only that file holds.
        if let Some(entity) = self
            .get_mut(id)
            .filter(|entity| entity.reading() && !entity.unread())
        {
            let saved = std::mem::take(&mut entity.saved.bytes);
            entity.saved = Baseline::read(saved, entity.saved.stamp);
        }
        let stamp = self.stamp();
        // The baseline stays, but whether the asset holds it may change. A revert, or an
        // edit made and then undone, puts back what it was saved as.
        let held = self
            .get(id)
            .is_some_and(|entity| entity.saved.holds(&bytes));
        let entity = self.get_mut(id)?;
        let (kept, link, wrote, pending) = (entity.kept, entity.link, entity.wrote, entity.pending);
        let path = entity.path.take();
        let saved = std::mem::take(&mut entity.saved);
        let saved = Baseline {
            stamp: match held {
                true => stamp,
                false => saved.stamp,
            },
            ..saved
        };
        let bytes = match held {
            true => saved.bytes.clone(),
            false => bytes,
        };
        let replaced =
            LocalEntity::new(id, entity.name.clone(), entity.origin.clone(), bytes, stamp);
        let verify = replaced.verify.clone();
        *entity = LocalEntity {
            kept,
            link,
            saved,
            wrote,
            pending,
            path,
            seen: entity.seen.clone(),
            ..replaced
        };
        Some(verify)
    }

    pub fn duplicate(&mut self, id: u64, log: &mut Log) -> Option<u64> {
        let source = self.get(id)?;
        let name = crate::strings::tagged(
            &source.name,
            &format!("{} copy", crate::strings::display_name(&source.name)),
        );
        let bytes = match source.whole() {
            Ok(bytes) => bytes.into_owned(),
            Err(e) => {
                log.error(format!("{}: {e}", source.name));
                log.trouble(format!(
                    "“{}” could not be read, so it was not copied.",
                    source.name
                ));
                return None;
            }
        };
        let origin = source.origin.clone();
        Some(self.ingest(name, origin, bytes, log))
    }

    pub fn remove(&mut self, id: u64, log: &mut Log) {
        if let Some(gone) = self.forget(id) {
            log.say(format!("Removed “{}” from this computer.", gone.name));
        }
    }

    /// Let go of an asset without a word, as when it turns out to be another asset's
    /// file.
    pub fn forget(&mut self, id: u64) -> Option<LocalEntity> {
        self.arriving.remove(&id);
        self.moving_over.remove(&id);
        self.saving_over.remove(&id);
        self.edits.remove(&id);
        let at = self.position(id)?;
        let gone = self.entities.remove(at);
        self.moved();
        Some(gone)
    }

    /// Let go of every asset on this computer, as another library takes its place, and
    /// return their ids. The views of slots stay.
    pub fn close_library(&mut self) -> Vec<u64> {
        let gone = self.listed().map(|entity| entity.id).collect();
        self.entities.retain(|entity| !entity.kept);
        self.let_go();
        self.moved();
        gone
    }

    /// Drop the checks, reads and decodes waiting on assets no longer held, so that the
    /// files' handles close.
    ///
    /// ⚠️ A check or read already running holds its file until it answers.
    fn let_go(&mut self) {
        let entities = &self.entities;
        let held = |id: u64| entities.iter().any(|entity| entity.id == id);
        self.checks.retain(|(id, _)| held(*id));
        self.edits.retain(|id, _| held(*id));
        self.moving_over.retain(|id, _| held(*id));
        self.saving_over.retain(|id, _| held(*id));
        self.undecoded.retain(|id| held(*id));
        self.hurried.get_mut().retain(|id| held(*id));
        self.wanted.get_mut().retain(|id, _| held(*id));
        self.asked.retain(|id| held(*id));
        if self.checking.as_ref().is_some_and(|check| !held(check.id)) {
            self.checking = None;
        }
        self.next_check();
    }

    /// The next id a new asset would take, for the store to carry over.
    pub fn next_id(&self) -> u64 {
        self.next_id
    }

    /// Give no new asset an id below `next`, which a library opening has handed out.
    pub fn reserve(&mut self, next: u64) {
        self.next_id = self.next_id.max(next);
    }

    /// Restore what a previous session held, and return how many assets were refused.
    ///
    /// Every asset is verified on the way in: bytes from a store have been somewhere
    /// this app does not control and get no more trust than bytes from a disk. An asset
    /// held whole is decoded and re-encoded off the frame, a few at a time, and is
    /// [`VerifyState::Reading`] until then; one holding an unsaved edit is decoded here.
    /// One resting in its file decodes nothing, and its checksum is checked off the
    /// frame. See [`Workspace::poll`]. One unread holds nothing until
    /// [`Workspace::took`]. An id is refused if it leaves no room for the next id or is
    /// already in the list.
    pub fn restore(&mut self, saved: Vec<Saved>, next_id: Option<u64>, log: &mut Log) -> usize {
        let mut refused = 0;
        // Kept current with each asset restored, so an id is looked up, not searched for.
        let mut held = std::mem::take(&mut *self.positions());
        for Saved {
            id,
            name,
            path,
            origin,
            saved,
            file,
            unread,
            unsaved,
        } in saved
        {
            let Some(next) = id.checked_add(1) else {
                refused += 1;
                continue;
            };
            // Every id held is below the next one, so only an id below it can be held.
            if id < self.next_id && held.1.contains_key(&id) {
                refused += 1;
                continue;
            }
            let stamp = self.stamp();
            let entity = match (file, unsaved, unread) {
                (None, None, Some(len)) => LocalEntity {
                    path,
                    ..LocalEntity::listed(id, name, origin, len, stamp)
                },
                (Some(file), None, _) => {
                    self.checks.push_back((id, file.clone()));
                    LocalEntity {
                        path,
                        ..LocalEntity::resting(id, name, origin, file, stamp)
                    }
                }
                (Some(file), Some(bytes), _) => {
                    let held = self.stamp();
                    self.checks.push_back((id, file.clone()));
                    LocalEntity {
                        saved: Baseline::on_disk(file, held),
                        path,
                        ..LocalEntity::new(id, name, origin, bytes.into(), stamp)
                    }
                }
                (None, None, None) => {
                    self.undecoded.push_back(id);
                    LocalEntity {
                        path,
                        ..LocalEntity::undecoded(id, name, origin, saved.into(), stamp)
                    }
                }
                (None, Some(bytes), Some(len)) => {
                    let saved = Baseline {
                        stamp: self.stamp(),
                        unread: Some(len),
                        ..Baseline::default()
                    };
                    LocalEntity {
                        saved,
                        path,
                        ..LocalEntity::new(id, name, origin, bytes.into(), stamp)
                    }
                }
                (None, Some(bytes), None) => {
                    // The saved and held bytes share a stamp, and are held once, only when
                    // they are the same bytes.
                    let saved = Bytes::from(saved);
                    let (held, bytes) = match saved == bytes {
                        true => (stamp, saved.clone()),
                        false => (self.stamp(), bytes.into()),
                    };
                    LocalEntity {
                        saved: Baseline::read(saved, held),
                        path,
                        ..LocalEntity::new(id, name, origin, bytes, stamp)
                    }
                }
            };
            if let Some(e) = entity.parse_error.as_ref().filter(|_| !entity.is_text) {
                log.warn(format!("{}: {e}", entity.name));
            }
            self.next_id = self.next_id.max(next);
            held.1.insert(id, self.entities.len());
            self.entities.push(entity);
        }
        if let Some(next) = next_id {
            self.next_id = self.next_id.max(next);
        }
        self.moved();
        held.0 = self.layout;
        *self.at.get_mut() = held;
        self.next_check();
        refused
    }

    /// Make one of the fresh defaults and add it to the list.
    pub fn create(&mut self, kind: Fresh, log: &mut Log) -> Option<u64> {
        match kind.bytes() {
            Ok(bytes) => {
                let name = format!("untitled.{}", kind.tag());
                Some(self.ingest(name, Origin::Fresh, bytes, log))
            }
            Err(e) => {
                log.error(format!("new {}: {e}", kind.label()));
                log.trouble(format!("Could not make a new {}.", kind.label()));
                None
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
async fn read_outside(from: &Outside) -> std::io::Result<Vec<u8>> {
    std::fs::read(from)
}

#[cfg(target_arch = "wasm32")]
async fn read_outside(from: &Outside) -> std::io::Result<Vec<u8>> {
    let buffer = wasm_bindgen_futures::JsFuture::from(from.array_buffer())
        .await
        .map_err(|e| std::io::Error::other(format!("{e:?}")))?;
    Ok(js_sys::Uint8Array::new(&buffer).to_vec())
}

/// Hand `bytes` to the user under `name`, however this target saves a file.
#[cfg(not(target_arch = "wasm32"))]
async fn save(name: String, bytes: Vec<u8>) -> Incoming {
    save_from(name, bytes.len() as u64, bytes.as_slice()).await
}

/// [`save`] for an asset resting in its file: the file is copied across, never held.
#[cfg(not(target_arch = "wasm32"))]
async fn save_file(name: String, file: Arc<OnDisk>) -> Incoming {
    save_from(name, file.len, file.reader()).await
}

#[cfg(not(target_arch = "wasm32"))]
async fn save_from(name: String, len: u64, source: impl std::io::Read) -> Incoming {
    let Some(handle) = rfd::AsyncFileDialog::new()
        .set_file_name(&name)
        .save_file()
        .await
    else {
        return Incoming::Note(format!("{name}: save canceled"));
    };
    match write_beside(handle.path(), source) {
        Ok(()) => Incoming::Note(format!("wrote {} ({len} bytes)", handle.file_name())),
        Err(e) => Incoming::Failed(format!("{name}: {e}")),
    }
}

/// Write what `source` yields to a sibling of `path` and rename it over `path`.
///
/// ⚠️ A library is hundreds of megabytes, and a write that stopped halfway would leave
/// the chosen file as neither the old nor the new one. The rename is the only moment the
/// chosen path changes, and the temp file is removed if anything fails.
#[cfg(not(target_arch = "wasm32"))]
fn write_beside(path: &std::path::Path, mut source: impl std::io::Read) -> std::io::Result<()> {
    let mut temp = path.as_os_str().to_owned();
    temp.push(".tmp");
    let temp = std::path::PathBuf::from(temp);
    let wrote = std::fs::File::create(&temp)
        .and_then(|mut out| std::io::copy(&mut source, &mut out))
        .and_then(|_| std::fs::rename(&temp, path));
    if wrote.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    wrote
}

/// The browser has no save picker: the bytes become a Blob behind an object URL, and
/// a synthetic anchor click hands that to the downloader.
#[cfg(target_arch = "wasm32")]
async fn save(name: String, bytes: Vec<u8>) -> Incoming {
    match download(&name, &bytes) {
        Ok(()) => Incoming::Note(format!("downloaded {name} ({} bytes)", bytes.len())),
        Err(e) => Incoming::Failed(format!("{name}: {e:?}")),
    }
}

/// [`save`] for an asset resting in its file: the browser hands the file over itself, and
/// nothing of it is read into this tab.
#[cfg(target_arch = "wasm32")]
fn save_file(name: &str, file: &OnDisk) -> Incoming {
    let Some(snapshot) = file.snapshot() else {
        return Incoming::Failed(format!("{name}: drawbar no longer reads its file"));
    };
    match hand_over(name, &snapshot) {
        Ok(()) => Incoming::Note(format!("downloaded {name} ({} bytes)", file.len)),
        Err(e) => Incoming::Failed(format!("{name}: {e:?}")),
    }
}

#[cfg(target_arch = "wasm32")]
fn download(name: &str, bytes: &[u8]) -> Result<(), wasm_bindgen::JsValue> {
    // `Uint8Array::from` copies into the JS heap, so the Blob does not alias Rust
    // memory that is about to be freed.
    let parts = js_sys::Array::new();
    parts.push(&js_sys::Uint8Array::from(bytes).into());
    let options = web_sys::BlobPropertyBag::new();
    options.set_type("application/octet-stream");
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &options)?;
    hand_over(name, &blob)
}

/// Hand `blob` to the downloader under `name`.
#[cfg(target_arch = "wasm32")]
fn hand_over(name: &str, blob: &web_sys::Blob) -> Result<(), wasm_bindgen::JsValue> {
    use wasm_bindgen::JsCast as _;
    use wasm_bindgen::JsValue;

    let document = web_sys::window()
        .and_then(|w| w.document())
        .ok_or_else(|| JsValue::from_str("no document"))?;
    let url = web_sys::Url::create_object_url_with_blob(blob)?;
    let anchor: web_sys::HtmlAnchorElement = document.create_element("a")?.unchecked_into();
    anchor.set_href(&url);
    anchor.set_download(name);
    anchor.click();
    web_sys::Url::revoke_object_url(&url)?;
    Ok(())
}

/// The same body under the shorter type-0 header the Electro 5's factory banks carry:
/// no CRC-32 word, and a CRC-16 over the whole file in the last two bytes.
#[cfg(test)]
pub(crate) fn as_type_0(bytes: &[u8]) -> Vec<u8> {
    let mut file = nord_usb::envelope::unwrap(bytes).expect("a CBIN file with a body");
    file.header.generation = Generation::V0;
    let mut out = std::io::Cursor::new(Vec::new());
    file.write_to(&mut out).expect("a type-0 container writes");
    out.into_inner()
}

/// Run a task that outlives the frame that started it.
///
/// ⚠️ wasm has one thread and cannot block: the future has to go to the microtask
/// queue, never to a thread, or the picker never resolves.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn spawn<F: std::future::Future<Output = ()> + Send + 'static>(future: F) {
    std::thread::spawn(move || nord_usb::block_on(future));
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn spawn<F: std::future::Future<Output = ()> + 'static>(future: F) {
    wasm_bindgen_futures::spawn_local(future);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file swapped, between its index and its check, for one of the same length in
    /// the other container generation is refused, not described by both.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn a_check_of_a_file_swapped_for_the_other_generation_is_refused() {
        use nord_format::cbin::{Cbin, RawBody};

        let dir = crate::testing::Temp::new();
        let bytes = crate::testing::sample_bytes();
        let file = crate::testing::on_disk(&dir, "Marimba.nsmp", &bytes);
        let indexed = file.index.header().clone();
        assert_eq!(indexed.generation, Generation::V1);

        let mut body = nord_usb::envelope::unwrap(&bytes).unwrap().body.0;
        body.resize(body.len() + 0x2c - 0x18 - 2, 0);
        let swapped = Cbin {
            header: Header {
                generation: Generation::V0,
                ..indexed
            },
            body: RawBody(body),
        };
        let mut other = std::io::Cursor::new(Vec::new());
        swapped.write_to(&mut other).unwrap();
        let other = other.into_inner();
        assert_eq!(other.len(), bytes.len(), "a same-length swap");
        std::fs::write(dir.at("Marimba.nsmp"), &other).unwrap();

        let ctx = crate::testing::context();
        let crate::work::Answer::Answered(Ok(sums)) = file.verify(&ctx).wait() else {
            panic!("the pass reads the swapped file");
        };
        assert!(sums.info.checksum_ok, "the swapped file is sound");
        let refused = Container::of_file(&file, sums).err();
        assert_eq!(
            refused.as_deref(),
            Some("the file changed while it was read")
        );
    }

    fn ingest(name: &str, bytes: Vec<u8>) -> LocalEntity {
        LocalEntity::new(1, name.into(), Origin::Fresh, bytes.into(), 0)
    }

    /// An edit made while the save of an earlier one was in flight outlives that save's
    /// answer: it is made again over the file the save wrote, still unsaved.
    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn an_edit_made_during_its_save_is_kept_over_the_file_the_save_wrote() {
        let dir = crate::testing::Temp::new();
        let bytes = crate::testing::sample_bytes();
        let file = crate::testing::on_disk(&dir, "Marimba.nsmp", &bytes);
        let mut workspace = Workspace::new(egui::Context::default());
        let id = crate::testing::rest(&mut workspace, "Marimba.nsmp", file);
        let named = |names: &[&str]| {
            let sets = names
                .iter()
                .map(|name| ("name".to_string(), name.to_string()));
            Edit::Sample(sets.collect())
        };
        workspace.hold_edit(id, Some(named(&["Vibes"])));
        assert!(workspace.save_edit(id));
        let (from, rewrite) = workspace.send_edit(id).expect("the store takes it");
        workspace.hold_edit(id, Some(named(&["Vibes", "Bells"])));

        let mut out = std::fs::File::create(dir.at("Saved.nsmp")).unwrap();
        rewrite.write(&from, &mut out).unwrap();
        drop(out);
        let saved = crate::testing::on_disk(&dir, "Saved.nsmp", &dir.read("Saved.nsmp"));
        workspace.edit_saved(id, saved.clone());

        assert_eq!(workspace.edit(id), Some(&named(&["Vibes", "Bells"])));
        let (over, _) = workspace.edit_of(id).expect("it applies to the saved file");
        assert!(Arc::ptr_eq(over, &saved));
        assert!(workspace.get(id).unwrap().is_unsaved());
        assert!(!workspace.saving_edit(id), "and waits to be saved again");
    }

    /// A send of a resting file that lands after the asset's edit was saved into another
    /// file leaves the asset resting in what the save wrote: the edit stays saved, and
    /// the write records the checksum of the file the slot took.
    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn a_send_landing_after_an_edit_was_saved_keeps_the_saved_edit() {
        let dir = crate::testing::Temp::new();
        let sent = crate::testing::on_disk(&dir, "Upright.npno", &crate::testing::piano(4));
        let edited = crate::testing::on_disk(&dir, "Edited.npno", &crate::testing::piano(5));
        let mut workspace = Workspace::new(egui::Context::default());
        let mut log = Log::default();
        let id = crate::testing::rest(&mut workspace, "Upright.npno", sent.clone());
        workspace.settle_files(&mut log);
        let crc32 = workspace.get(id).unwrap().saved.crc32.expect("checked");

        workspace.edit_saved(id, edited.clone());
        workspace.settle_files(&mut log);
        let (class, at) = (ObjectClass::Piano, Location::from_user(1, 1));
        workspace.landed_file(id, class, at, sent, crc32);

        let entity = workspace.get(id).unwrap();
        assert!(entity
            .rests()
            .is_some_and(|file| Arc::ptr_eq(file, &edited)));
        assert!(!entity.is_unsaved(), "the edit is saved");
        assert_eq!(entity.link, Some((class, at)));
        assert!(entity.wrote.is_some_and(|wrote| wrote.crc32 == crc32));
        assert_ne!(entity.saved.crc32, Some(crc32), "so the slot is behind");
    }

    /// Closing a library lets go of the files its assets rested in, checks still waiting
    /// their turn included, so no handle to the library it left stays open.
    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn closing_the_library_lets_go_of_the_files_its_assets_rest_in() {
        let dir = crate::testing::Temp::new();
        let bytes = crate::testing::sample_bytes();
        let mut workspace = Workspace::new(egui::Context::default());
        let files: Vec<Arc<OnDisk>> = ["First.nsmp", "Second.nsmp", "Third.nsmp"]
            .into_iter()
            .map(|name| {
                let file = crate::testing::on_disk(&dir, name, &bytes);
                crate::testing::rest(&mut workspace, name, file.clone());
                file
            })
            .collect();
        let waiting = &files[2];
        assert!(Arc::strong_count(waiting) > 2, "its check waits its turn");

        workspace.close_library();
        assert_eq!(workspace.listed().count(), 0);
        assert_eq!(Arc::strong_count(waiting), 1, "only this test holds it");
    }

    /// The number a file and a slot are compared on is the CRC-32 of the wire body, and
    /// the word a type-1 header stores at `0x18` is that same number.
    #[test]
    fn the_body_checksum_is_the_word_a_type_1_header_stores() {
        let bytes = Fresh::Program.bytes().unwrap();
        let entity = ingest("untitled.ne5p", bytes.clone());
        let container = entity
            .container
            .as_ref()
            .expect("a fresh program is a CBIN file");
        assert_eq!(container.header.generation, Generation::V1);
        let body = nord_usb::envelope::unwrap(&bytes).expect("a file the wire takes");
        let hashed = nord_format::crc::crc32(&body.body.0);
        assert_eq!(container.body_crc32, hashed);
        assert_eq!(
            container.body_crc32,
            u32::from_le_bytes(bytes[0x18..0x1c].try_into().unwrap()),
        );
        assert_eq!(entity.saved.crc32, Some(hashed));
    }

    /// ⚠️ A type-0 container stores no body checksum, only a CRC-16 over the whole file.
    /// The number the instrument reports for a slot is computed from the body, not read
    /// from the header, which is the only way an Electro 5 factory program can be
    /// compared with the slot holding it.
    #[test]
    fn a_type_0_file_has_the_body_checksum_its_header_does_not_carry() {
        let bytes = as_type_0(&Fresh::Program.bytes().unwrap());
        let entity = ingest("Circling Bells.ne5p", bytes.clone());
        let container = entity.container.as_ref().expect("still a CBIN file");
        assert_eq!(container.header.generation, Generation::V0);
        assert!(container.checksum_ok);
        assert_eq!(container.checksum_label, "crc16:");

        let body = nord_usb::envelope::unwrap(&bytes).expect("a file the wire takes");
        let hashed = nord_format::crc::crc32(&body.body.0);
        assert_eq!(container.body_crc32, hashed);
        assert_eq!(entity.saved.crc32, Some(hashed));
    }

    /// Each fresh default carries its own tag and round-trips. That is all the New menu
    /// claims about a zeroed body: it decodes and re-saves byte for byte.
    #[test]
    fn every_fresh_default_round_trips_under_its_own_tag() {
        for kind in Fresh::ALL.iter().filter(|kind| **kind != Fresh::Text) {
            let entity = ingest("untitled", kind.bytes().unwrap());
            assert!(entity.parse_error.is_none(), "{kind:?}");
            assert_eq!(entity.tag(), kind.tag(), "{kind:?}");
            assert!(matches!(entity.verify, VerifyState::Ok), "{kind:?}");
            assert!(
                entity.container.expect("a CBIN file").checksum_ok,
                "{kind:?}"
            );
        }
    }

    #[test]
    fn a_new_note_is_an_empty_file_that_is_already_a_note() {
        let entity = ingest("untitled.txt", Fresh::Text.bytes().unwrap());
        assert!(entity.bytes.is_empty());
        assert!(entity.container.is_none(), "a note is under no container");
        assert_eq!(entity.tag(), Fresh::Text.tag());
        assert_eq!(
            crate::browser::Kind::of(&entity),
            crate::browser::Kind::Text
        );
    }

    /// Every kind is on the New menu once: a kind on no menu cannot be found, and one on
    /// two is offered twice.
    #[test]
    fn every_kind_is_offered_exactly_once() {
        let mut seen: Vec<Fresh> = Fresh::FAMILIES
            .iter()
            .flat_map(|family| family.kinds.iter().copied())
            .chain(Fresh::LOOSE)
            .collect();
        assert_eq!(seen.len(), Fresh::ALL.len());
        for kind in Fresh::ALL {
            let at = seen.iter().position(|held| *held == kind);
            seen.remove(at.unwrap_or_else(|| panic!("{kind:?} is on no menu")));
        }
        assert!(seen.is_empty());
    }

    /// A zeroed body is not a factory program, and the menu says so before the user
    /// finds out.
    #[test]
    fn a_zeroed_body_says_that_it_is_one() {
        assert!(!Fresh::Program.zeroed() && Fresh::Program.note().is_none());
        for kind in Fresh::ALL.iter().filter(|kind| kind.zeroed()) {
            let note = kind.note().unwrap_or_else(|| panic!("{kind:?}"));
            assert!(note.contains("zero"), "{kind:?}: {note}");
        }
    }

    /// Two tags the same would be two menu entries making the same file.
    #[test]
    fn no_two_kinds_share_a_tag() {
        let mut tags: Vec<&str> = Fresh::ALL.iter().map(|kind| kind.tag()).collect();
        tags.sort_unstable();
        let held = tags.len();
        tags.dedup();
        assert_eq!(tags.len(), held);
    }

    /// ⚠️ `Africa Split.ne5p copy` puts the tag in the middle of the name, where nothing
    /// reads it: an export then stacks a second one on the end.
    #[test]
    fn a_duplicate_is_a_copy_of_the_name_under_the_same_tag() {
        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx);
        let mut log = Log::default();
        for (name, copied, exported) in [
            ("untitled.txt", "untitled copy.txt", "untitled-copy.txt"),
            (
                "Africa Split.ne5p",
                "Africa Split copy.ne5p",
                "Africa-Split-copy.ne5p",
            ),
            // No tag to keep, so the export takes one from the bytes, which are words.
            (
                "no tag at all",
                "no tag at all copy",
                "no-tag-at-all-copy.txt",
            ),
        ] {
            let id = workspace.ingest(
                name.to_string(),
                Origin::Fresh,
                b"Set 1\n".to_vec(),
                &mut log,
            );
            let copy = workspace
                .duplicate(id, &mut log)
                .expect("it is on the list");
            let copy = workspace.get(copy).expect("the copy");
            assert_eq!(copy.name, copied);
            assert_eq!(
                export_filename(&copy.name, &copy.bytes),
                exported,
                "and an export does not stack a second tag on it"
            );
        }
    }

    /// ⚠️ Whether an asset is text is computed once, when its bytes land. A value that
    /// outlived its bytes would leave a document editing a file that is no longer there.
    #[test]
    fn whether_an_asset_is_words_follows_its_bytes() {
        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx);
        let mut log = Log::default();
        let id = workspace.ingest("held".into(), Origin::Fresh, b"Set 1\n".to_vec(), &mut log);
        assert!(workspace.get(id).expect("held").is_text);

        workspace.replace_bytes(id, vec![0x00, 0xff, 0x01, 0xfe], &mut log);
        let held = workspace.get(id).expect("held");
        assert!(!held.is_text, "these bytes are no longer words");
        assert_eq!(crate::browser::Kind::of(held), crate::browser::Kind::Other);

        workspace.revert(id, &mut log);
        assert!(workspace.get(id).expect("held").is_text, "and back again");
    }

    /// A slot opened for a look is a working copy that nothing lists, and it goes when
    /// the tab looking at it does.
    #[test]
    fn a_view_is_not_on_this_computer_until_it_is_kept() {
        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx);
        let mut log = Log::default();
        let at = Location { bank: 6, slot: 3 };
        let device = || Origin::Device {
            class: ObjectClass::Program,
            at,
        };

        let viewed = workspace.view(
            "Africa-Split.ne5p".into(),
            device(),
            Fresh::Program.bytes().unwrap(),
            &mut log,
        );
        let copied = workspace.ingest(
            "Squabble-B.ne5p".into(),
            device(),
            Fresh::Program.bytes().unwrap(),
            &mut log,
        );

        assert!(workspace.is_view(viewed) && !workspace.is_view(copied));
        let listed: Vec<u64> = workspace.listed().map(|e| e.id).collect();
        assert_eq!(listed, vec![copied], "a view is not in the local list");
        // It is still an entity in every other way: a tab and a send both find it.
        assert!(workspace.get(viewed).is_some());
        assert_eq!(workspace.entities().len(), 2);

        // Edited, it stays a view; kept, it stops being one and keeps its edit.
        let edited = workspace.get(viewed).unwrap().bytes.to_vec();
        workspace.replace_bytes(viewed, [edited, vec![]].concat(), &mut log);
        assert!(workspace.is_view(viewed));
        workspace.keep(viewed, &mut log);
        assert!(!workspace.is_view(viewed));
        assert_eq!(workspace.listed().count(), 2);
    }

    /// ⚠️ A view is not on this computer, and the activity log records where a slot's
    /// bytes went, so the log must not say it was kept.
    #[test]
    fn viewing_a_slot_says_that_and_not_that_it_was_kept() {
        let mut workspace = Workspace::new(egui::Context::default());
        let mut log = Log::default();
        let id = workspace.view(
            "Africa-Split.ne5p".into(),
            Origin::Device {
                class: ObjectClass::Program,
                at: Location { bank: 6, slot: 3 },
            },
            Fresh::Program.bytes().unwrap(),
            &mut log,
        );

        assert!(workspace.is_view(id));
        assert!(log.status().1.starts_with("Viewing "), "{}", log.status().1);
        assert!(
            !log.iter()
                .any(|entry| entry.text.contains("is on this computer.")),
            "a view was never taken onto this computer"
        );
        // The detail of what arrived is still recorded, view or not.
        assert!(log.iter().any(|entry| entry.text.contains("verified")));
    }

    /// ⚠️ A write is what this app knows about a slot without reading it back, and the
    /// only basis for the Agrees mark on a class whose slots report no checksum. An edit
    /// and its revert do not touch the slot, so they must keep that evidence.
    #[test]
    fn an_edit_and_a_revert_leave_the_write_this_app_made() {
        let mut workspace = Workspace::new(egui::Context::default());
        let mut log = Log::default();
        let at = Location { bank: 6, slot: 3 };
        let id = workspace.create(Fresh::Program, &mut log).unwrap();
        let sent = workspace.get(id).unwrap().bytes.to_vec();
        workspace.landed(id, ObjectClass::Program, at, sent.clone());
        let wrote = |workspace: &Workspace| {
            workspace
                .get(id)
                .unwrap()
                .wrote
                .map(|held| (held.class, held.at, held.crc32))
        };
        let landed = wrote(&workspace).expect("a write this app made");

        let (_, edited) =
            crate::fields::apply(&sent, &[("center_panel.gain".into(), "96".into())]).unwrap();
        workspace.replace_bytes(id, edited, &mut log);
        assert_eq!(wrote(&workspace), Some(landed), "an edit is not a write");

        workspace.revert(id, &mut log);
        assert_eq!(wrote(&workspace), Some(landed), "and neither is a revert");
    }

    /// A view is dropped once no tab holds it. A kept asset stays whether or not a tab
    /// shows it.
    #[test]
    fn a_view_goes_when_the_last_tab_on_it_closes() {
        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx);
        let mut log = Log::default();
        let at = Location { bank: 6, slot: 3 };
        let viewed = workspace.view(
            "Africa-Split.ne5p".into(),
            Origin::Device {
                class: ObjectClass::Program,
                at,
            },
            Fresh::Program.bytes().unwrap(),
            &mut log,
        );
        let local = workspace.create(Fresh::Program, &mut log).unwrap();

        let queue = Queue::default();
        workspace.close_views(|id| id == viewed, &queue, &mut log);
        assert!(workspace.get(viewed).is_some(), "its tab is still open");

        workspace.close_views(|_| false, &queue, &mut log);
        assert!(workspace.get(viewed).is_none());
        assert!(workspace.get(local).is_some(), "kept is kept");
    }

    /// ⚠️ A view is the only copy of what it holds: nothing lists it and the store skips
    /// it. The × on its tab sits next to a badge saying the edit is owed to a slot, with
    /// no undo. An edited or owed view is kept; only an untouched one is dropped.
    #[test]
    fn a_view_with_changes_in_it_is_kept_rather_than_dropped() {
        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx);
        let mut log = Log::default();
        let at = |slot| Location { bank: 6, slot };
        let view = |workspace: &mut Workspace, slot, log: &mut Log| {
            workspace.view(
                format!("view-{slot}.ne5p"),
                Origin::Device {
                    class: ObjectClass::Program,
                    at: at(slot),
                },
                Fresh::Program.bytes().unwrap(),
                log,
            )
        };

        let edited = view(&mut workspace, 0, &mut log);
        let owed = view(&mut workspace, 1, &mut log);
        let untouched = view(&mut workspace, 2, &mut log);

        let bytes = workspace.get(edited).unwrap().bytes.to_vec();
        workspace.replace_bytes(edited, [bytes, vec![0]].concat(), &mut log);
        let mut queue = Queue::default();
        crate::queue::enqueue(
            &workspace,
            &mut crate::device::Device::new(workspace.ctx().clone()),
            &mut queue,
            &mut log,
            owed,
            ObjectClass::Program,
            at(1),
        );
        assert!(precious(workspace.get(edited).unwrap(), &queue));
        assert!(precious(workspace.get(owed).unwrap(), &queue));
        assert!(!precious(workspace.get(untouched).unwrap(), &queue));

        // Every tab closes at once.
        workspace.close_views(|_| false, &queue, &mut log);

        assert!(workspace.get(untouched).is_none(), "the slot still has it");
        let listed: Vec<u64> = workspace.listed().map(|e| e.id).collect();
        assert_eq!(listed, vec![edited, owed], "and the changes survive");
        assert!(!workspace.is_view(edited) && !workspace.is_view(owed));
        // Promoting it does not take it out of the queue.
        assert!(queue.holds(owed));
        assert!(log.status().1.contains("kept on this computer"));
    }

    /// ⚠️ An edit not yet applied to the bytes is the same loss: a piano library's plan
    /// is held by the document, not the file, and dropping the view would silently
    /// discard it.
    #[test]
    fn a_view_whose_edit_is_still_a_plan_is_kept_rather_than_dropped() {
        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx);
        let mut log = Log::default();
        let planning = workspace.view(
            "Africa-Split.ne5p".into(),
            Origin::Device {
                class: ObjectClass::Program,
                at: Location { bank: 6, slot: 3 },
            },
            Fresh::Program.bytes().unwrap(),
            &mut log,
        );
        let queue = Queue::default();
        assert!(!precious(workspace.get(planning).unwrap(), &queue));

        workspace.mark_pending(planning, true);
        assert!(precious(workspace.get(planning).unwrap(), &queue));
        workspace.close_views(|_| false, &queue, &mut log);
        assert!(workspace.get(planning).is_some(), "the plan survives");
        assert!(!workspace.is_view(planning), "and is listed");
    }

    /// ⚠️ A library is hundreds of megabytes. The chosen file is replaced only by a
    /// rename, so a write that stops halfway leaves the old file, and no temporary file is
    /// left beside it either way.
    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn a_save_lands_whole_and_leaves_no_temporary_beside_it() {
        let dir = std::env::temp_dir().join("drawbar-save-beside");
        std::fs::create_dir_all(&dir).expect("a directory to save into");
        let path = dir.join("Royal Grand.npno");
        std::fs::write(&path, b"what was there before").expect("a file to replace");

        write_beside(&path, &b"the bytes the editor made"[..]).expect("it writes");
        assert_eq!(
            std::fs::read(&path).expect("it is there"),
            b"the bytes the editor made"
        );
        let left: Vec<std::ffi::OsString> = std::fs::read_dir(&dir)
            .expect("it is still a directory")
            .map(|entry| entry.expect("an entry").file_name())
            .collect();
        assert_eq!(left, [path.file_name().expect("a name")], "{left:?}");

        let nowhere = dir.join("no-such-folder").join("Royal Grand.npno");
        assert!(write_beside(&nowhere, &b"anything"[..]).is_err());
        assert!(!nowhere.with_extension("npno.tmp").exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// One view per slot, so a second double-click goes to the existing view instead of
    /// making a second working copy.
    #[test]
    fn a_slot_has_at_most_one_view() {
        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx);
        let mut log = Log::default();
        let at = Location { bank: 6, slot: 3 };
        let elsewhere = Location { bank: 6, slot: 4 };
        let class = ObjectClass::Program;

        assert_eq!(workspace.view_of(class, at), None);
        let id = workspace.view(
            "Africa-Split.ne5p".into(),
            Origin::Device { class, at },
            Fresh::Program.bytes().unwrap(),
            &mut log,
        );
        assert_eq!(workspace.view_of(class, at), Some(id));
        assert_eq!(workspace.view_of(class, elsewhere), None);
        assert_eq!(workspace.view_of(ObjectClass::SetList, at), None);

        // A copy on this computer is not a view of the slot, however it got here.
        workspace.ingest(
            "Africa-Split.ne5p".into(),
            Origin::Device { class, at },
            Fresh::Program.bytes().unwrap(),
            &mut log,
        );
        assert_eq!(workspace.view_of(class, at), Some(id));
        // And once the view is kept, the slot has none.
        workspace.keep(id, &mut log);
        assert_eq!(workspace.view_of(class, at), None);
    }

    /// The raw-body export is the file without its container: for a type-1 file,
    /// everything from `body_start` on.
    #[test]
    fn the_raw_body_export_drops_the_container_header() {
        let entity = ingest("untitled.ne5p", Fresh::Program.bytes().unwrap());
        let body = entity.raw_body().expect("a CBIN file has a body");
        assert_eq!(body.len(), ne5::program::BODY_LEN);
        assert_eq!(
            body.as_slice(),
            &entity.bytes[Generation::V1.body_start() as usize..],
        );
    }

    /// The name is kept as written everywhere; only the export dialog sees a path-safe
    /// form, with the extension taken from the bytes when the name carries none.
    #[test]
    fn an_export_sanitizes_the_name_and_supplies_the_extension() {
        let bytes = Fresh::Program.bytes().unwrap();
        let file = |name: &str| export_filename(name, &bytes);
        assert_eq!(file("Big strings"), "Big-strings.ne5p");
        assert_eq!(file("patch.ne5p"), "patch.ne5p", "a carried tag is kept");
        assert_eq!(file("Bass 2.0"), "Bass-2.0.ne5p", "a dot is not a tag");
        assert_eq!(file("../../etc/passwd"), "etc-passwd.ne5p");
        assert_eq!(file("  "), "unnamed.ne5p");
        assert_eq!(
            export_filename("Big strings", &[0x00, 0xff, 0x01, 0xfe]),
            "Big-strings.bin",
        );
        assert_eq!(
            export_filename("Set 1", b"Set 1\n"),
            "Set-1.txt",
            "words are a note, and a note is exported as one"
        );
    }

    #[test]
    fn a_zone_wav_is_named_after_its_instrument_and_number() {
        assert_eq!(zone_wav_name("Bass Clarinet", 2), "Bass-Clarinet-zone2.wav");
        assert_eq!(zone_wav_name("../../etc/passwd", 1), "etc-passwd-zone1.wav");
        assert_eq!(zone_wav_name("  ", 1), "unnamed-zone1.wav");
    }

    #[test]
    fn bytes_carry_a_stamp_that_changes_only_when_they_do() {
        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx);
        let mut log = Log::default();

        let id = workspace.create(Fresh::Program, &mut log).unwrap();
        let opened = workspace.get(id).unwrap().bytes.to_vec();
        let stamp = |workspace: &Workspace| workspace.get(id).unwrap().stamp;
        let first = stamp(&workspace);

        workspace.rename(id, "Africa-Split".into());
        assert_eq!(stamp(&workspace), first, "the bytes did not move");

        let (_, edited) =
            crate::fields::apply(&opened, &[("center_panel.gain".into(), "96".into())]).unwrap();
        workspace.replace_bytes(id, edited, &mut log);
        let second = stamp(&workspace);
        assert_ne!(second, first);

        workspace.revert(id, &mut log);
        let third = stamp(&workspace);
        assert_ne!(third, second);
        assert_ne!(third, first, "back to the same bytes is still a new decode");

        // Reverting to the bytes already held changes nothing, stamp included.
        workspace.revert(id, &mut log);
        assert_eq!(stamp(&workspace), third);
    }

    /// Unsaved is holding bytes other than the ones this asset was last saved as. An
    /// edit makes it so, saving and reverting each end it.
    #[test]
    fn an_asset_is_unsaved_while_it_holds_something_its_baseline_does_not() {
        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx);
        let mut log = Log::default();

        let id = workspace.create(Fresh::Program, &mut log).unwrap();
        let opened = workspace.get(id).unwrap().bytes.to_vec();
        let unsaved = |workspace: &Workspace| workspace.get(id).unwrap().is_unsaved();
        assert!(!unsaved(&workspace), "a fresh asset starts saved");

        let (_, edited) =
            crate::fields::apply(&opened, &[("center_panel.gain".into(), "96".into())]).unwrap();
        workspace.replace_bytes(id, edited.clone(), &mut log);
        assert!(unsaved(&workspace));
        assert_eq!(workspace.get(id).unwrap().saved.bytes, opened);

        // Saving moves the baseline onto what it holds; the bytes do not move.
        workspace.mark_saved(id);
        assert!(!unsaved(&workspace));
        assert_eq!(workspace.get(id).unwrap().saved.bytes, edited);
        assert_eq!(workspace.get(id).unwrap().bytes, edited);

        // Reverting moves the bytes back onto the baseline, which stays where it is.
        let (_, again) =
            crate::fields::apply(&edited, &[("center_panel.gain".into(), "12".into())]).unwrap();
        workspace.replace_bytes(id, again, &mut log);
        assert!(unsaved(&workspace));
        workspace.revert(id, &mut log);
        assert!(!unsaved(&workspace));
        assert_eq!(workspace.get(id).unwrap().bytes, edited);

        // An edit undone by hand is back at the baseline too.
        let (_, away) =
            crate::fields::apply(&edited, &[("center_panel.gain".into(), "12".into())]).unwrap();
        workspace.replace_bytes(id, away, &mut log);
        assert!(unsaved(&workspace));
        workspace.replace_bytes(id, edited, &mut log);
        assert!(!unsaved(&workspace), "it holds what it was saved as again");
    }

    /// ⚠️ The other half of unsaved. A piano library's plan is an edit its bytes do not
    /// hold, because nothing is copied until something needs the bytes. An asset reading
    /// as saved while a plan stood would offer no revert, show no star, and be discarded
    /// with the view it was edited in.
    #[test]
    fn an_asset_is_unsaved_while_an_editor_holds_an_edit_its_bytes_do_not() {
        let mut workspace = Workspace::new(egui::Context::default());
        let mut log = Log::default();
        let queue = Queue::default();

        let id = workspace.create(Fresh::Program, &mut log).unwrap();
        assert!(!workspace.get(id).unwrap().is_unsaved());

        assert!(workspace.mark_pending(id, true), "the edit is news");
        assert!(!workspace.mark_pending(id, true), "and is news only once");
        let held = workspace.get(id).unwrap();
        assert!(held.is_unsaved());
        assert_eq!(held.bytes, held.saved.bytes, "with no body copied for it");
        assert!(precious(held, &queue));

        workspace.revert(id, &mut log);
        assert!(
            !workspace.get(id).unwrap().is_unsaved(),
            "and reverting an edit the bytes never held only drops the edit",
        );
        assert!(
            log.status().1.contains("back as it was last saved"),
            "{}",
            log.status().1,
        );
    }

    /// A write that reached a slot saves the bytes it carried, not the ones the asset
    /// holds now: an edit made while the write was in flight exists only on this
    /// computer, and calling it saved would let it be discarded with its tab.
    #[test]
    fn a_write_that_landed_saves_what_it_carried_rather_than_a_later_edit() {
        let mut workspace = Workspace::new(egui::Context::default());
        let mut log = Log::default();
        let at = Location { bank: 6, slot: 3 };
        let id = workspace.create(Fresh::Program, &mut log).unwrap();
        let sent = workspace.get(id).unwrap().bytes.to_vec();

        let (_, edited) =
            crate::fields::apply(&sent, &[("center_panel.gain".into(), "96".into())]).unwrap();
        workspace.replace_bytes(id, edited.clone(), &mut log);
        workspace.landed(id, ObjectClass::Program, at, sent.clone());
        assert!(
            workspace.get(id).unwrap().is_unsaved(),
            "the edit made in flight is still owed"
        );
        assert_eq!(workspace.get(id).unwrap().saved.bytes, sent);

        workspace.landed(id, ObjectClass::Program, at, edited);
        assert!(
            !workspace.get(id).unwrap().is_unsaved(),
            "a write of what it holds leaves nothing owed"
        );
    }

    /// A project is text, so the bytes at the CBIN tag offset mean nothing. The export
    /// must not read a tag out of the prose, and the long extension counts as carried.
    #[test]
    fn a_project_export_keeps_its_own_extension() {
        let project = nord_format::formats::nsmpproj::Project::new(
            "One",
            &[nord_format::formats::nsmpproj::NewZone {
                path: "one.wav".into(),
                sample_rate: 44100,
                frames: 44100,
                root_key: 60,
            }],
            0,
        )
        .unwrap();
        let bytes = nord_format::to_bytes(&Entity::SampleProject(project)).unwrap();
        assert_eq!(
            export_filename("proj.nsmpproj", &bytes),
            "proj.nsmpproj",
            "a carried project extension is kept"
        );
        assert_eq!(export_filename("My Kit", &bytes), "My-Kit.nsmpproj");
    }

    /// A file that does not decode is still a row: the error is the report.
    #[test]
    fn bytes_that_do_not_decode_are_kept_with_their_error() {
        let entity = ingest("junk.bin", vec![0x00, 0xff, 0x01, 0xfe]);
        assert!(entity.entity.is_none());
        assert!(entity.parse_error.is_some());
        assert!(entity.container.is_none());
        assert!(matches!(entity.verify, VerifyState::NotApplicable(_)));
        assert_eq!(entity.tag(), "?");
    }

    #[test]
    fn a_malformed_sample_editor_project_keeps_its_error_and_is_not_a_note() {
        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx);
        let mut log = Log::default();
        let bytes = b"SMACEditorProject {\n  m_fileFormatVersion = oops\n".to_vec();
        let id = workspace.ingest("broken.nsmpproj".into(), Origin::Fresh, bytes, &mut log);

        let held = workspace.get(id).expect("held");
        let error = held
            .parse_error
            .clone()
            .expect("the project did not decode");
        assert!(!held.is_text);
        assert_eq!(crate::browser::Kind::of(held), crate::browser::Kind::Other);
        assert!(
            log.iter()
                .any(|entry| entry.level == crate::log::Level::Error && entry.text.contains(&error)),
            "the decode error is logged"
        );
    }

    #[test]
    fn words_longer_than_a_note_holds_stay_a_record() {
        let words = "Set 1\n".repeat(crate::document::text::MAX_BYTES / 6 + 1);
        let held = ingest("a long log.txt", words.into_bytes());
        assert!(!held.is_text);
        assert_eq!(crate::browser::Kind::of(&held), crate::browser::Kind::Other);
        assert!(matches!(held.verify, VerifyState::NotApplicable(_)));
    }

    /// The name is this app's metadata and the only record of what an object is called,
    /// since a file stores none. It must survive the whole way: off the instrument, into
    /// a tab, through an edit, and out to a filename.
    #[test]
    fn a_name_survives_being_fetched_opened_edited_and_exported() {
        use nord_usb::{Location, ObjectClass};

        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx);
        let mut log = Log::default();
        let at = Location { bank: 6, slot: 3 };

        // As a read from the instrument arrives: only the device supplies the name.
        let id = workspace.ingest(
            "Africa-Split.ne5p".into(),
            Origin::Device {
                class: ObjectClass::Program,
                at,
            },
            Fresh::Program.bytes().unwrap(),
            &mut log,
        );
        assert_eq!(workspace.get(id).unwrap().name, "Africa-Split.ne5p");

        // An edit: new bytes, same name.
        let bytes = workspace.get(id).unwrap().bytes.to_vec();
        let (_, edited) =
            crate::fields::apply(&bytes, &[("center_panel.gain".into(), "96".into())]).unwrap();
        workspace.replace_bytes(id, edited, &mut log);
        assert_eq!(workspace.get(id).unwrap().name, "Africa-Split.ne5p");
        assert!(workspace.get(id).unwrap().is_unsaved());

        // Reverting is not renaming either.
        workspace.revert(id, &mut log);
        assert_eq!(workspace.get(id).unwrap().bytes, bytes);
        assert_eq!(workspace.get(id).unwrap().name, "Africa-Split.ne5p");

        // The filename an export offers is that same name, not one derived from the
        // bytes.
        assert_eq!(
            workspace.export_name(id).as_deref(),
            Some("Africa-Split.ne5p")
        );
    }

    /// What a previous session held comes back keeping its name, its origin and its id,
    /// and is decoded and checked off the frame.
    #[test]
    fn a_restored_asset_keeps_what_it_was() {
        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx);
        let mut log = Log::default();
        workspace.restore(
            vec![Saved {
                id: 9,
                name: "Africa-Split.ne5p".into(),
                path: None,
                origin: Origin::Fresh,
                saved: Fresh::Program.bytes().unwrap(),
                file: None,
                unread: None,
                unsaved: None,
            }],
            Some(10),
            &mut log,
        );
        let entity = workspace.get(9).expect("restored under its own id");
        assert_eq!(entity.name, "Africa-Split.ne5p");
        assert!(entity.reading());
        assert_eq!(
            crate::browser::Kind::of(entity),
            crate::browser::Kind::Program,
            "what its name says"
        );
        assert!(!entity.is_unsaved());
        workspace.settle_files(&mut log);
        let entity = workspace.get(9).unwrap();
        assert!(matches!(entity.verify, VerifyState::Ok));
        assert_eq!(
            crate::browser::Kind::of(entity),
            crate::browser::Kind::Program
        );
        assert!(!entity.is_unsaved());
        // A new asset cannot land on an id something restored is already using.
        let fresh = workspace.create(Fresh::Live, &mut log).unwrap();
        assert!(fresh >= 10);
    }

    /// An id already held is refused, in the same restore or a later one; a free id
    /// below the next one is not.
    #[test]
    fn a_restore_refuses_only_the_ids_already_held() {
        let mut workspace = Workspace::new(egui::Context::default());
        let mut log = Log::default();
        let saved = |id| Saved {
            id,
            name: format!("{id}.ne5p"),
            path: None,
            origin: Origin::Fresh,
            saved: Fresh::Program.bytes().unwrap(),
            file: None,
            unread: None,
            unsaved: None,
        };
        assert_eq!(
            workspace.restore(vec![saved(5), saved(5)], None, &mut log),
            1
        );
        assert_eq!(
            workspace.restore(vec![saved(5), saved(3)], None, &mut log),
            1
        );
        let ids: Vec<u64> = workspace.listed().map(|entity| entity.id).collect();
        assert_eq!(ids, [5, 3]);
    }

    /// A restore in parts, as a listing brings a library back, looks each id up rather
    /// than searching the list for it, however many assets it already holds.
    #[test]
    fn a_restore_in_parts_never_searches_the_list_per_asset() {
        const PARTS: u64 = 8;
        const PART: u64 = 256;
        let mut workspace = Workspace::new(egui::Context::default());
        let mut log = Log::default();
        let listed = |id| Saved {
            id,
            name: format!("{id}.ne5p"),
            path: None,
            origin: Origin::Fresh,
            saved: Vec::new(),
            file: None,
            unread: Some(10),
            unsaved: None,
        };
        // Ids arrive out of order, each below the next id the listing has given out.
        for part in 0..PARTS {
            let ids = (0..PART).map(|n| 1 + n * PARTS + part);
            let refused =
                workspace.restore(ids.map(listed).collect(), Some(1 + PARTS * PART), &mut log);
            assert_eq!(refused, 0);
        }
        assert_eq!(workspace.listed().count() as u64, PARTS * PART);
        let searched = workspace.searched.get();
        assert!(searched <= PARTS as usize, "searched {searched} times");
    }

    /// The assets asked for first are decoded first, whatever order the rest arrived in.
    #[test]
    fn an_asset_hurried_is_decoded_before_the_others() {
        let mut workspace = Workspace::new(egui::Context::default());
        let mut log = Log::default();
        let bytes = Fresh::Program.bytes().unwrap();
        let held = DECODES * DECODE.0 + 1;
        let saved = (1..=held as u64).map(|id| Saved {
            id,
            name: format!("{id}.ne5p"),
            path: None,
            origin: Origin::Fresh,
            saved: bytes.clone(),
            file: None,
            unread: None,
            unsaved: None,
        });
        workspace.restore(saved.collect(), None, &mut log);
        let last = held as u64;
        workspace.hurry(last);
        workspace.next_decodes();
        assert!(
            workspace.flying.contains(&last),
            "the last to arrive goes first"
        );
        assert!(
            !workspace.flying.contains(&(last - 1)),
            "one more than fits waits"
        );
        workspace.settle_files(&mut log);
        assert_eq!(workspace.reading(), 0);
    }

    /// Unread assets 1 to `n`, each `len` bytes long, in the root of the library.
    fn unread_assets(n: u64, len: u64) -> Vec<Saved> {
        (1..=n)
            .map(|id| Saved {
                id,
                name: format!("Sound {id:03}.ne5p"),
                path: Some(LibPath::root().join(&format!("Sound {id:03}.ne5p"))),
                origin: Origin::Fresh,
                saved: Vec::new(),
                file: None,
                unread: Some(len),
                unsaved: None,
            })
            .collect()
    }

    #[test]
    fn the_library_is_asked_for_what_is_open_then_selected_then_in_view() {
        let mut workspace = Workspace::new(egui::Context::default());
        workspace.restore(unread_assets(100, 1), None, &mut Log::default());
        workspace.in_view(1..=60);
        workspace.selected(70);
        workspace.hurry(80);
        workspace.in_view([80]);

        assert_eq!(workspace.take_wanted(), [80], "open, alone");
        assert_eq!(workspace.take_wanted(), [70], "then selected");
        let rows: Vec<u64> = (1..=60).collect();
        assert_eq!(
            workspace.take_wanted(),
            rows[..READ.0],
            "then a read of rows"
        );
        assert_eq!(workspace.take_wanted(), rows[READ.0..]);
        assert!(!workspace.wants());
    }

    #[test]
    fn a_read_of_large_files_in_view_stops_at_its_bytes() {
        let mut workspace = Workspace::new(egui::Context::default());
        let len = READ.1 / 2;
        workspace.restore(unread_assets(5, len), None, &mut Log::default());
        workspace.in_view(1..=5);
        assert_eq!(workspace.take_wanted(), [1, 2]);
        assert_eq!(workspace.take_wanted(), [3, 4]);
        assert_eq!(workspace.take_wanted(), [5]);
    }

    #[test]
    fn a_row_scrolled_out_of_view_before_its_read_is_not_read() {
        let mut workspace = Workspace::new(egui::Context::default());
        let mut log = Log::default();
        workspace.restore(unread_assets(10, 1), None, &mut log);
        workspace.in_view(1..=5);
        workspace.selected(9);
        workspace.poll(&mut log);
        workspace.in_view(4..=8);
        workspace.poll(&mut log);

        assert_eq!(workspace.take_wanted(), [9], "selected stays wanted");
        assert_eq!(
            workspace.take_wanted(),
            [4, 5, 6, 7, 8],
            "the rows drawn last frame"
        );
        for id in 1..=3 {
            assert!(!workspace.wanted(id), "row {id} left the view");
        }
        assert!(!workspace.wants());
        workspace.in_view([2]);
        assert_eq!(workspace.take_wanted(), [2], "back in view, wanted again");
    }

    /// A note read from the library, or restored with an unsaved edit, is words and not
    /// a file that failed to decode, so it logs no problem.
    #[test]
    fn a_note_read_from_the_library_logs_no_problem() {
        let mut workspace = Workspace::new(egui::Context::default());
        let mut log = Log::default();
        let words = b"Smoke note 1\n".to_vec();
        let note = |id: u64, unread: Option<u64>, unsaved: Option<Vec<u8>>| Saved {
            id,
            name: format!("Note {id}.txt"),
            path: Some(LibPath::root().join(&format!("Note {id}.txt"))),
            origin: Origin::Fresh,
            saved: match unread {
                Some(_) => Vec::new(),
                None => words.clone(),
            },
            file: None,
            unread,
            unsaved,
        };
        let edited = b"Smoke note 1, edited\n".to_vec();
        let saved = vec![
            note(1, Some(words.len() as u64), None),
            note(2, None, Some(edited)),
        ];
        workspace.restore(saved, None, &mut log);
        workspace.hurry(1);
        assert_eq!(workspace.take_wanted(), vec![1]);
        workspace.took(1, Some(words), None);
        workspace.settle_files(&mut log);

        for id in [1, 2] {
            let entity = workspace.get(id).expect("listed");
            assert!(entity.is_text, "Note {id} is a note");
        }
        assert_eq!(log.problems(), 0, "{:?}", log.status());
    }

    /// A flipped body byte is reported: the container's checksum no longer matches, and
    /// the decode fails.
    #[test]
    fn a_tampered_body_byte_is_reported() {
        let mut bytes = Fresh::Program.bytes().unwrap();
        let at = Generation::V1.body_start() as usize + 0x30;
        bytes[at] ^= 0xff;
        let entity = ingest("tampered.ne5p", bytes);
        assert!(entity.parse_error.is_some());
        assert!(!entity.container.expect("still a CBIN file").checksum_ok);
    }
}
