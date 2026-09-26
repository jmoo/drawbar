//! This computer: the assets held in memory, and the ways bytes get in and out of them.
//!
//! An entity is bytes first and a decode second. A file that does not parse still gets a
//! row, with its error shown and its raw body exportable, because reporting a bad file is
//! the point of opening it.
//!
//! [`crate::browser`] draws the list; this module holds the model and the file dialogs.

use std::sync::mpsc::{Receiver, Sender};

use eframe::egui;
use nord_format::cbin::{Cbin, Generation, Header};
use nord_format::formats::{ne5, ns2, ns3, ns4, nsmpproj};
use nord_format::{Entity, Live, OrganPreset, PianoPreset, Program, Settings, Song, Synth};
use nord_usb::{Location, ObjectClass};

use crate::log::Log;
use crate::newproject::{Draft, Making};
use crate::queue::Queue;

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
    /// Offset of the first byte that came back different.
    Differs {
        at: usize,
    },
    /// Re-encoding refused.
    Failed(String),
    /// Nothing to check, and why.
    NotApplicable(&'static str),
}

impl VerifyState {
    pub fn badge(&self) -> &'static str {
        match self {
            VerifyState::Ok => "ok",
            VerifyState::Differs { .. } => "differs",
            VerifyState::Failed(_) => "failed",
            VerifyState::NotApplicable(_) => "n/a",
        }
    }

    pub fn detail(&self) -> String {
        match self {
            VerifyState::Ok => "re-encoded byte-for-byte".into(),
            VerifyState::Differs { at } => format!("first difference at byte {at:#06x}"),
            VerifyState::Failed(why) => why.clone(),
            VerifyState::NotApplicable(why) => (*why).to_string(),
        }
    }

    pub fn color(&self, visuals: &egui::Visuals) -> egui::Color32 {
        match self {
            VerifyState::Ok => crate::app::good(visuals),
            VerifyState::Differs { .. } | VerifyState::Failed(_) => crate::app::bad(visuals),
            VerifyState::NotApplicable(_) => visuals.weak_text_color(),
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
    /// The CRC-32 of the wire body, which the device reports for a slot, so a file and
    /// the slot it came off compare without hashing either body again.
    ///
    /// Computed, not read: a type-1 container stores the same number at `0x18`, but a
    /// type-0 container stores only a CRC-16 over the whole file.
    pub body_crc32: u32,
}

impl Container {
    fn read(bytes: &[u8]) -> Option<Container> {
        let info = nord_format::cbin::inspect(&mut std::io::Cursor::new(bytes)).ok()?;
        let start = usize::try_from(info.header.generation.body_start()).ok()?;
        let end = start.checked_add(usize::try_from(info.body_len).ok()?)?;
        let body = start..end;
        let body_crc32 = nord_usb::envelope::crc32(bytes.get(body.clone())?);
        // `Header` omits the generation-specific checksum field, so the stored value is
        // read here for display.
        let (checksum_label, checksum) = match info.header.generation {
            Generation::V0 => {
                let tail = bytes.get(bytes.len().checked_sub(2)?..)?;
                let crc = u16::from_le_bytes(tail.try_into().ok()?);
                ("crc16:", format!("{crc:#06x}"))
            }
            Generation::V1 => {
                let crc = u32::from_le_bytes(bytes.get(0x18..0x1c)?.try_into().ok()?);
                ("crc32:", format!("{crc:#010x}"))
            }
        };
        Some(Container {
            header: info.header,
            body,
            checksum_ok: info.checksum_ok,
            checksum_label,
            checksum,
            body_crc32,
        })
    }

    pub fn tag(&self) -> String {
        String::from_utf8_lossy(&self.header.tag).into_owned()
    }

    /// The body's length in bytes.
    pub fn body_len(&self) -> u64 {
        self.body.len() as u64
    }
}

/// What an asset was last saved as: the bytes, and the checksum a slot holding them
/// would report.
///
/// ⚠️ The checksum is computed when the baseline moves and never per frame. Computing one
/// streams the whole body, and every listed row asks for it while the library is shown.
#[derive(Clone, Default)]
pub struct Baseline {
    pub bytes: Vec<u8>,
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
}

impl Baseline {
    /// The baseline of bytes not yet inspected, stamped with `stamp`.
    pub(crate) fn read(bytes: Vec<u8>, stamp: u64) -> Baseline {
        let crc32 = Container::read(&bytes).map(|held| held.body_crc32);
        Baseline {
            bytes,
            crc32,
            stamp,
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

/// One object held in memory: its bytes, what they decode to, and how they got here.
pub struct LocalEntity {
    /// Stable across reordering, so a selection survives a removal.
    pub id: u64,
    pub name: String,
    pub origin: Origin,
    pub bytes: Vec<u8>,
    pub entity: Option<Entity>,
    pub parse_error: Option<String>,
    pub container: Option<Container>,
    /// Whether the bytes are a note, from [`crate::document::text::is_text`].
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
}

impl LocalEntity {
    fn new(id: u64, name: String, origin: Origin, bytes: Vec<u8>, stamp: u64) -> LocalEntity {
        let container = Container::read(&bytes);
        let (entity, parse_error) =
            match nord_format::from_stream(&mut std::io::Cursor::new(&bytes)) {
                Ok(entity) => (Some(entity), None),
                Err(e) => (None, Some(e.to_string())),
            };
        let verify = match &entity {
            Some(entity) => verify(entity, &bytes),
            None => VerifyState::NotApplicable("the file did not decode"),
        };
        let is_text = crate::document::text::is_text(&bytes);
        let mut held = LocalEntity {
            id,
            name,
            origin,
            bytes,
            entity,
            parse_error,
            container,
            is_text,
            verify,
            saved: Baseline::default(),
            pending: false,
            kept: true,
            stamp,
            link: None,
            wrote: None,
        };
        held.saved = held.baseline();
        held
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
        }
    }

    /// The slot this asset stands for: its link, or else the slot it came off.
    pub fn spot(&self) -> Option<(ObjectClass, Location)> {
        self.link.or_else(|| self.origin.slot())
    }

    /// The format tag, from the decode if there is one and from the container otherwise.
    ///
    /// A note has neither, so its tag comes from its bytes being text. See
    /// [`crate::document::text::is_text`].
    pub fn tag(&self) -> String {
        match (&self.entity, &self.container) {
            (Some(entity), _) => entity.identity().format.to_string(),
            (None, Some(container)) => container.tag(),
            (None, None) if self.is_text => crate::document::text::EXTENSION.to_string(),
            (None, None) => "?".into(),
        }
    }

    /// The bytes the wire would carry: the file with its container stripped.
    ///
    /// The counterpart of `nord … get --body` pointed at a file. `None` for anything
    /// that is not a CBIN container.
    pub fn raw_body(&self) -> Option<Vec<u8>> {
        nord_usb::envelope::unwrap(&self.bytes)
            .ok()
            .map(|read| read.body.0)
    }
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
    let stem = match filename_stem(name) {
        s if s.is_empty() => "unnamed".to_string(),
        s => s,
    };
    match crate::strings::carries_tag(&stem) {
        true => stem,
        false => format!("{stem}.{}", format_tag(bytes)),
    }
}

/// The filename for a decoded zone's WAV: the instrument's name made path-safe, and the
/// zone numbered as the document numbers it.
///
/// `nord sample decode --out` writes the same name, so a zone exported from either tool
/// gets one name.
pub fn zone_wav_name(instrument: &str, zone: usize) -> String {
    let stem = match filename_stem(instrument) {
        s if s.is_empty() => "unnamed".to_string(),
        s => s,
    };
    format!("{stem}-zone{zone}.wav")
}

/// The filename for a decoded piano stroke's WAV: the library's name made path-safe,
/// and the stroke named as `nord piano decode` names it, `<root>-b<bank>-l<layer>`, with
/// the MIDI note zero-padded to three digits.
pub fn stroke_wav_name(library: &str, root: u8, bank: u8, layer: u8) -> String {
    let stem = match filename_stem(library) {
        s if s.is_empty() => "unnamed".to_string(),
        s => s,
    };
    format!("{stem}-{root:03}-b{bank}-l{layer:02}.wav")
}

/// A name reduced to what a path can carry: runs of whitespace, dashes, and path
/// separators become one `-`, control characters are dropped, and leading or trailing
/// dots and dashes are trimmed so the file is neither hidden nor option-like. For
/// filenames only; the name itself is never changed.
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
    out.trim_matches(['.', '-']).to_string()
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
    match out.iter().zip(bytes).position(|(a, b)| a != b) {
        Some(at) => VerifyState::Differs { at },
        None if out.len() == bytes.len() => VerifyState::Ok,
        // A shared prefix and a different length: they differ where the shorter ends.
        None => VerifyState::Differs {
            at: out.len().min(bytes.len()),
        },
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
            Fresh::Program
            | Fresh::Live
            | Fresh::SetList
            | Fresh::Settings
            | Fresh::Stage2Program
            | Fresh::Stage3Program
            | Fresh::Stage3Synth
            | Fresh::Stage4Program
            | Fresh::Stage4Organ
            | Fresh::Stage4Piano
            | Fresh::Stage4Synth => self.zeroed().then_some(
                "Every control at zero. The file decodes and re-saves byte for byte, but \
                 it is not a factory program. This app does not know what one would hold.",
            ),
        }
    }

    /// The file this makes, as [`Workspace::create`] adds it to the list.
    pub(crate) fn bytes(self) -> Result<Vec<u8>, String> {
        let at = |slot: u16| -> Result<ne5::program::Location, String> {
            (0, slot).try_into().map_err(|e| format!("{e}"))
        };
        let entity = match self {
            Fresh::Program => Entity::Program(Program::Electro5(ne5::program::new(at(0)?))),
            Fresh::Live => Entity::Live(Live::Electro5(ne5::live::new(
                (0, 0).try_into().map_err(|e| format!("{e}"))?,
            ))),
            // A set list is only four program pointers, so it starts with the first four
            // programs.
            Fresh::SetList => Entity::Song(Song::Electro5(
                ne5::song::new(
                    (0, 0).try_into().map_err(|e| format!("{e}"))?,
                    ne5::song::DEFAULT_VERSION,
                    [at(0)?, at(1)?, at(2)?, at(3)?],
                )
                .map_err(|e| format!("{e}"))?,
            )),
            Fresh::Settings => Entity::Settings(Settings::Electro5(ne5::settings::new())),
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
    pub origin: Origin,
    /// What it was last saved as.
    pub saved: Vec<u8>,
    /// What it holds now, if that differs from what it was saved as.
    pub unsaved: Option<Vec<u8>>,
}

/// What a background task hands back to the UI thread.
enum Incoming {
    Opened {
        name: String,
        bytes: Vec<u8>,
    },
    /// Every file one pick of WAVs returned, together, because the draft asks one
    /// question about the whole set.
    Wavs {
        making: Making,
        files: Vec<(String, Vec<u8>)>,
    },
    Note(String),
    Failed(String),
}

pub struct Workspace {
    entities: Vec<LocalEntity>,
    next_id: u64,
    /// Bumped by every change to the list, so the shell can tell when the store is
    /// behind without comparing every asset's bytes.
    revision: u64,
    ctx: egui::Context,
    tx: Sender<Incoming>,
    rx: Receiver<Incoming>,
    /// The WAVs a New pick came back with, waiting on their root keys. See
    /// [`crate::newproject`].
    draft: Option<Draft>,
}

impl Workspace {
    pub fn new(ctx: egui::Context) -> Workspace {
        let (tx, rx) = std::sync::mpsc::channel();
        Workspace {
            entities: Vec::new(),
            next_id: 1,
            revision: 0,
            ctx,
            tx,
            rx,
            draft: None,
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
        self.entities.iter().find(|e| e.id == id)
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
        let Some(entity) = self.entities.iter_mut().find(|e| e.id == id) else {
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
        let Some(entity) = self.entities.iter_mut().find(|e| e.id == id) else {
            return;
        };
        if std::mem::replace(&mut entity.kept, true) {
            return;
        }
        let name = entity.name.clone();
        self.revision += 1;
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
        self.revision += 1;
    }

    /// Recompute every asset's link. Call whenever the instrument's scan cache changes,
    /// with [`crate::device::link`] as `held_by`.
    pub fn relink(&mut self, held_by: impl Fn(&LocalEntity) -> Option<(ObjectClass, Location)>) {
        for entity in &mut self.entities {
            entity.link = held_by(entity);
        }
    }

    /// Rename an asset held here. Nothing leaves this computer.
    pub fn rename(&mut self, id: u64, name: String) {
        if let Some(entity) = self.entities.iter_mut().find(|e| e.id == id) {
            entity.name = name;
            self.revision += 1;
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
        let entity = LocalEntity::new(id, name, origin, bytes, self.stamp());
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
        (id, arrival)
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
            .map(|entity| (entity.stamp, entity.bytes == bytes))
        {
            Some((stamp, true)) => stamp,
            _ => self.stamp(),
        }
    }

    /// Drain whatever the pickers finished with. Call once per frame.
    pub fn poll(&mut self, log: &mut Log) {
        while let Ok(message) = self.rx.try_recv() {
            match message {
                Incoming::Opened { name, bytes } => {
                    self.ingest(name.clone(), Origin::File(name), bytes, log);
                }
                Incoming::Wavs { making, files } => self.draft = Draft::plan(making, files),
                Incoming::Note(text) => log.say(text),
                Incoming::Failed(text) => log.trouble(text),
            }
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
                let bytes = handle.read().await;
                let _ = tx.send(Incoming::Opened {
                    name: handle.file_name(),
                    bytes,
                });
            }
            ctx.request_repaint();
        });
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
        Some(export_filename(&entity.name, &entity.bytes))
    }

    pub fn export(&self, id: u64) {
        let Some(entity) = self.entities.iter().find(|e| e.id == id) else {
            return;
        };
        let name = match self.export_name(id) {
            Some(name) => name,
            None => return,
        };
        self.save_bytes(name, entity.bytes.clone());
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
        let Some(saved) = self.get(id).map(|entity| entity.saved.bytes.clone()) else {
            return;
        };
        let dropped = self.mark_pending(id, false);
        if self.respell(id, saved).is_none() && !dropped {
            return;
        }
        if let Some(entity) = self.get(id) {
            log.say(format!("“{}” is back as it was last saved.", entity.name));
        }
    }

    /// Record whether an editor holds an edit of this asset that its bytes do not, and
    /// return whether that changed. See [`LocalEntity::is_unsaved`].
    ///
    /// ⚠️ Only the editor holding the edit can tell, so it must call this on every frame
    /// the answer might change. The bytes are the saved ones either way.
    pub fn mark_pending(&mut self, id: u64, pending: bool) -> bool {
        let Some(entity) = self.entities.iter_mut().find(|e| e.id == id) else {
            return false;
        };
        if std::mem::replace(&mut entity.pending, pending) == pending {
            return false;
        }
        self.revision += 1;
        true
    }

    /// Make the current bytes the saved baseline.
    ///
    /// ⚠️ The bytes do not change, so the stamp stays and caches over them stay valid.
    /// The list revision moves, so the store is written again without the unsaved copy.
    pub fn mark_saved(&mut self, id: u64) {
        let Some(entity) = self.entities.iter_mut().find(|e| e.id == id) else {
            return;
        };
        if !entity.is_unsaved() {
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
        let stamp = self.stamp_for(id, &sent);
        let Some(entity) = self.entities.iter_mut().find(|e| e.id == id) else {
            return;
        };
        entity.saved = Baseline::read(sent, stamp);
        entity.link = Some((class, at));
        entity.wrote = entity.saved.crc32.map(|crc32| Wrote { class, at, crc32 });
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
        let Some(verify) = self.respell(id, bytes) else {
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
    fn respell(&mut self, id: u64, bytes: Vec<u8>) -> Option<VerifyState> {
        if self.get(id).is_none_or(|entity| entity.bytes == bytes) {
            return None;
        }
        let stamp = self.stamp();
        // The baseline stays, but whether the asset holds it may change. A revert, or an
        // edit made and then undone, puts back what it was saved as.
        let held = self
            .get(id)
            .is_some_and(|entity| entity.saved.bytes == bytes);
        let entity = self.entities.iter_mut().find(|e| e.id == id)?;
        let (kept, link, wrote, pending) = (entity.kept, entity.link, entity.wrote, entity.pending);
        let saved = std::mem::take(&mut entity.saved);
        let saved = Baseline {
            stamp: match held {
                true => stamp,
                false => saved.stamp,
            },
            ..saved
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
            ..replaced
        };
        Some(verify)
    }

    pub fn duplicate(&mut self, id: u64, log: &mut Log) -> Option<u64> {
        let source = self.entities.iter().find(|e| e.id == id)?;
        let name = crate::strings::tagged(
            &source.name,
            &format!("{} copy", crate::strings::display_name(&source.name)),
        );
        let (origin, bytes) = (source.origin.clone(), source.bytes.clone());
        Some(self.ingest(name, origin, bytes, log))
    }

    pub fn remove(&mut self, id: u64, log: &mut Log) {
        let Some(at) = self.entities.iter().position(|e| e.id == id) else {
            return;
        };
        let gone = self.entities.remove(at);
        self.revision += 1;
        log.say(format!("Removed “{}” from this computer.", gone.name));
    }

    /// The next id a new asset would take, for the store to carry over.
    pub fn next_id(&self) -> u64 {
        self.next_id
    }

    /// Restore what a previous session held, and return how many assets were refused.
    ///
    /// Every asset is decoded and verified on the way in: bytes from a store have been
    /// somewhere this app does not control and get no more trust than bytes from a disk.
    /// An id is refused if it leaves no room for the next id or is already in the list.
    ///
    /// ⚠️ Restore decodes and re-encodes every asset before the first wasm frame. The
    /// tab cannot yield while checking up to the store budget.
    pub fn restore(&mut self, saved: Vec<Saved>, next_id: Option<u64>, log: &mut Log) -> usize {
        let mut refused = 0;
        for Saved {
            id,
            name,
            origin,
            saved,
            unsaved,
        } in saved
        {
            let Some(next) = id.checked_add(1) else {
                refused += 1;
                continue;
            };
            if self.entities.iter().any(|e| e.id == id) {
                refused += 1;
                continue;
            }
            let stamp = self.stamp();
            let bytes = unsaved.unwrap_or_else(|| saved.clone());
            // The saved and held bytes share a stamp only when they are the same bytes.
            let held = match bytes == saved {
                true => stamp,
                false => self.stamp(),
            };
            let entity = LocalEntity {
                saved: Baseline::read(saved, held),
                ..LocalEntity::new(id, name, origin, bytes, stamp)
            };
            if let Some(e) = &entity.parse_error {
                log.warn(format!("{}: {e}", entity.name));
            }
            self.next_id = self.next_id.max(next);
            self.entities.push(entity);
        }
        if let Some(next) = next_id {
            self.next_id = self.next_id.max(next);
        }
        self.revision += 1;
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

/// Hand `bytes` to the user under `name`, however this target saves a file.
#[cfg(not(target_arch = "wasm32"))]
async fn save(name: String, bytes: Vec<u8>) -> Incoming {
    let Some(handle) = rfd::AsyncFileDialog::new()
        .set_file_name(&name)
        .save_file()
        .await
    else {
        return Incoming::Note(format!("{name}: save canceled"));
    };
    match write_beside(handle.path(), &bytes) {
        Ok(()) => Incoming::Note(format!(
            "wrote {} ({} bytes)",
            handle.file_name(),
            bytes.len(),
        )),
        Err(e) => Incoming::Failed(format!("{name}: {e}")),
    }
}

/// Write `bytes` to a sibling of `path` and rename it over `path`.
///
/// ⚠️ A library is hundreds of megabytes, and a write that stopped halfway would leave
/// the chosen file as neither the old nor the new one. The rename is the only moment the
/// chosen path changes, and the temp file is removed if anything fails.
#[cfg(not(target_arch = "wasm32"))]
fn write_beside(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut temp = path.as_os_str().to_owned();
    temp.push(".tmp");
    let temp = std::path::PathBuf::from(temp);
    let wrote = std::fs::write(&temp, bytes).and_then(|()| std::fs::rename(&temp, path));
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

#[cfg(target_arch = "wasm32")]
fn download(name: &str, bytes: &[u8]) -> Result<(), wasm_bindgen::JsValue> {
    use wasm_bindgen::JsCast as _;
    use wasm_bindgen::JsValue;

    let document = web_sys::window()
        .and_then(|w| w.document())
        .ok_or_else(|| JsValue::from_str("no document"))?;

    // `Uint8Array::from` copies into the JS heap, so the Blob does not alias Rust
    // memory that is about to be freed.
    let parts = js_sys::Array::new();
    parts.push(&js_sys::Uint8Array::from(bytes).into());
    let options = web_sys::BlobPropertyBag::new();
    options.set_type("application/octet-stream");
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &options)?;

    let url = web_sys::Url::create_object_url_with_blob(&blob)?;
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
fn spawn<F: std::future::Future<Output = ()> + Send + 'static>(future: F) {
    std::thread::spawn(move || nord_usb::block_on(future));
}

#[cfg(target_arch = "wasm32")]
fn spawn<F: std::future::Future<Output = ()> + 'static>(future: F) {
    wasm_bindgen_futures::spawn_local(future);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ingest(name: &str, bytes: Vec<u8>) -> LocalEntity {
        LocalEntity::new(1, name.into(), Origin::Fresh, bytes, 0)
    }

    #[test]
    fn a_fresh_program_decodes_and_verifies() {
        let entity = ingest("untitled.ne5p", Fresh::Program.bytes().unwrap());
        assert!(entity.parse_error.is_none());
        assert_eq!(entity.tag(), "ne5p");
        assert!(
            matches!(entity.verify, VerifyState::Ok),
            "{}",
            entity.verify.detail()
        );

        let container = entity.container.expect("a fresh program is a CBIN file");
        assert!(container.checksum_ok);
        assert_eq!(container.header.generation, Generation::V1);
        assert_eq!(container.body.len(), ne5::program::BODY_LEN);
        assert_eq!(container.checksum_label, "crc32:");
    }

    /// The number a file and a slot are compared on is the CRC-32 of the wire body, and
    /// the word a type-1 header stores at `0x18` is that same number.
    #[test]
    fn the_body_checksum_is_the_word_a_type_1_header_stores() {
        let bytes = Fresh::Program.bytes().unwrap();
        let entity = ingest("untitled.ne5p", bytes.clone());
        let container = entity.container.expect("a fresh program is a CBIN file");
        assert_eq!(container.header.generation, Generation::V1);
        let body = nord_usb::envelope::unwrap(&bytes).expect("a file the wire takes");
        assert_eq!(
            container.body_crc32,
            nord_usb::envelope::crc32(&body.body.0)
        );
        assert_eq!(
            container.body_crc32,
            u32::from_le_bytes(bytes[0x18..0x1c].try_into().unwrap()),
        );
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
        let hashed = nord_usb::envelope::crc32(&body.body.0);
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
        let edited = workspace.get(viewed).unwrap().bytes.clone();
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
        let sent = workspace.get(id).unwrap().bytes.clone();
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

        let bytes = workspace.get(edited).unwrap().bytes.clone();
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

        write_beside(&path, b"the bytes the editor made").expect("it writes");
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
        assert!(write_beside(&nowhere, b"anything").is_err());
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
        let opened = workspace.get(id).unwrap().bytes.clone();
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
        let opened = workspace.get(id).unwrap().bytes.clone();
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
        let sent = workspace.get(id).unwrap().bytes.clone();

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

    /// The baseline's checksum is the one a slot holding those bytes reports, so a saved
    /// asset and its slot are compared without either body being hashed again.
    #[test]
    fn the_baseline_carries_the_checksum_a_slot_holding_it_reports() {
        let entity = ingest("untitled.ne5p", Fresh::Program.bytes().unwrap());
        let body = nord_usb::envelope::unwrap(&entity.bytes).expect("a file the wire takes");
        assert_eq!(
            entity.saved.crc32,
            Some(nord_usb::envelope::crc32(&body.body.0))
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
        let bytes = workspace.get(id).unwrap().bytes.clone();
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

    /// What a previous session held comes back decoded and checked, keeping its name,
    /// its origin and its id.
    #[test]
    fn a_restored_asset_keeps_what_it_was() {
        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx);
        let mut log = Log::default();
        workspace.restore(
            vec![Saved {
                id: 9,
                name: "Africa-Split.ne5p".into(),
                origin: Origin::Fresh,
                saved: Fresh::Program.bytes().unwrap(),
                unsaved: None,
            }],
            Some(10),
            &mut log,
        );
        let entity = workspace.get(9).expect("restored under its own id");
        assert_eq!(entity.name, "Africa-Split.ne5p");
        assert!(matches!(entity.verify, VerifyState::Ok));
        assert!(!entity.is_unsaved());
        // A new asset cannot land on an id something restored is already using.
        let fresh = workspace.create(Fresh::Live, &mut log).unwrap();
        assert!(fresh >= 10);
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
