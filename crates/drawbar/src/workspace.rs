//! This computer: the assets held in memory, and the ways bytes get in and out of them.
//!
//! An entity is bytes first and a decode second — a file that does not parse still
//! gets a row, with its error and its raw body still exportable, because reporting a
//! bad file is the point of opening it.
//!
//! The list draws itself in [`crate::browser`]; what lives here is the model and the
//! file dialogs.

use std::sync::mpsc::{Receiver, Sender};

use eframe::egui;
use nord_format::cbin::{Cbin, Generation, Header};
use nord_format::formats::{ne5, ns2, ns3, ns4};
use nord_format::{Entity, Live, OrganPreset, PianoPreset, Program, Settings, Song, Synth};
use nord_usb::{Location, ObjectClass};

use crate::log::Log;

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

    /// The slot this came off, for the tab header's way back.
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
/// ⚠️ Reading them streams the whole file to check the checksum, so it happens on the
/// way in and never per frame — a piano library is hundreds of megabytes.
#[derive(Clone)]
pub struct Container {
    pub header: Header,
    pub body_len: u64,
    pub checksum_ok: bool,
    /// `crc32:` or `crc16:` — the two generations keep it in different places.
    pub checksum_label: &'static str,
    pub checksum: String,
}

impl Container {
    fn read(bytes: &[u8]) -> Option<Container> {
        let info = nord_format::cbin::inspect(&mut std::io::Cursor::new(bytes)).ok()?;
        // The one header fact the parsed `Header` does not carry: a type-1 file holds
        // a crc32 over its body at 0x18, a type-0 file a crc16 over the whole file in
        // its last two bytes.
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
            body_len: info.body_len,
            checksum_ok: info.checksum_ok,
            checksum_label,
            checksum,
        })
    }

    pub fn tag(&self) -> String {
        String::from_utf8_lossy(&self.header.tag).into_owned()
    }
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
    pub verify: VerifyState,
    pub dirty: bool,
    /// Owed back to the slot it came from, waiting on a Send.
    pub pending: bool,
    /// Whether this is on this computer, as opposed to a view of a slot.
    ///
    /// A view is a working copy like any other — it is edited and sent back the same
    /// way — but it is not in the local list and goes when its tab does. Only
    /// [`Workspace::keep`] promotes one.
    pub kept: bool,
}

impl LocalEntity {
    fn new(id: u64, name: String, origin: Origin, bytes: Vec<u8>) -> LocalEntity {
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
        LocalEntity {
            id,
            name,
            origin,
            bytes,
            entity,
            parse_error,
            container,
            verify,
            dirty: false,
            pending: false,
            kept: true,
        }
    }

    /// The format tag, from the decode where there is one and the container otherwise.
    pub fn tag(&self) -> String {
        match (&self.entity, &self.container) {
            (Some(entity), _) => entity.identity().format.to_string(),
            (None, Some(container)) => container.tag(),
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
/// The one rule that decides what happens to a view when the last thing looking at it
/// goes: **an edited or owed view is precious, an untouched one is disposable.** An
/// untouched view is the slot's own bytes, which the instrument still has; an edited one
/// is the only copy there is.
pub fn precious(entity: &LocalEntity) -> bool {
    entity.dirty || entity.pending
}

/// The filename an export suggests for a verbatim name: made path-safe, and given the
/// extension the bytes say it should have unless the name already carries one.
fn export_filename(name: &str, bytes: &[u8], what: ExportWhat) -> String {
    let stem = match filename_stem(name) {
        s if s.is_empty() => "unnamed".to_string(),
        s => s,
    };
    match what {
        ExportWhat::Body => match stem.ends_with(".body") {
            true => stem,
            false => format!("{stem}.body"),
        },
        ExportWhat::File => match carries_tag(&stem) {
            true => stem,
            false => format!("{stem}.{}", format_tag(bytes)),
        },
    }
}

/// A verbatim name reduced to what a path can carry: whitespace runs and path
/// separators become one `-`, control characters drop, and nothing hidden or
/// option-like survives at the edges. Filenames only — the name itself stays verbatim.
fn filename_stem(label: &str) -> String {
    // A separator is owed rather than written, so a run of them collapses to one `-`
    // and nothing trailing survives.
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

/// Whether a name already ends in something shaped like a format tag (`patch.ne5p`,
/// `x.body`), so an export must not stack a second one on it.
fn carries_tag(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(stem, tag)| {
        !stem.trim().is_empty()
            && (2..=5).contains(&tag.len())
            && tag.chars().all(|c| c.is_ascii_alphanumeric())
            && tag.chars().any(|c| c.is_ascii_alphabetic())
    })
}

/// The extension a nameless export gets: the CBIN tag the bytes themselves carry, or
/// `bin` for bytes that carry none.
fn format_tag(bytes: &[u8]) -> String {
    bytes
        .get(8..12)
        .filter(|tag| tag.iter().all(|b| b.is_ascii_alphanumeric()))
        .map(|tag| String::from_utf8_lossy(tag).into_owned())
        .unwrap_or_else(|| "bin".to_string())
}

/// Re-encode and compare — the check `nord verify` runs on a file.
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
        // A shared prefix and a different length: they part at the end of the shorter.
        None => VerifyState::Differs {
            at: out.len().min(bytes.len()),
        },
    }
}

/// One product family and the objects it can be started from nothing, which is how the
/// New menu is arranged: a family, then a kind inside it.
pub struct Family {
    pub label: &'static str,
    pub kinds: &'static [Fresh],
}

/// A body of every zero is a legal body: each field's type decodes the whole of its
/// slot, so nothing in it is out of range. Build one under the newest version the
/// decode is validated against.
///
/// ⚠️ Legal is not the same as **default**. What comes out is every control at zero,
/// which is not a state any panel ships in — see [`Fresh::zeroed`], which is what puts
/// that in front of the operator before they make one.
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
/// ⚠️ Only bodies that **decode** are here. A stub format — the Stage 3's song, the
/// settings of any Stage — round-trips its container and nothing more, so a zeroed one
/// is 45 bytes of nothing under a tag rather than an object, and offering it would put a
/// file in front of the operator that this app cannot say a single true thing about.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fresh {
    /// The Electro 5's four, each from the library's own constructor.
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
}

impl Fresh {
    pub const ALL: [Fresh; 11] = [
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
    ];

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
        }
    }

    /// Whether what this makes is a zeroed body rather than a constructed object.
    ///
    /// The distinction the menu has to carry: an Electro 5 program comes from the
    /// library's own constructor, and a Stage anything is every control at zero.
    pub fn zeroed(self) -> bool {
        !matches!(
            self,
            Fresh::Program | Fresh::Live | Fresh::SetList | Fresh::Settings
        )
    }

    /// The sentence a hover puts on the menu entry, where there is something the
    /// operator would otherwise have to find out by opening the file.
    pub fn note(self) -> Option<&'static str> {
        self.zeroed().then_some(
            "Every control at zero. The file decodes and re-saves byte for byte, but it \
             is not a factory program — nothing here knows what one would hold.",
        )
    }

    fn bytes(self) -> Result<Vec<u8>, String> {
        let at = |slot: u16| -> Result<ne5::program::Location, String> {
            (0, slot).try_into().map_err(|e| format!("{e}"))
        };
        let entity = match self {
            Fresh::Program => Entity::Program(Program::Electro5(ne5::program::new(at(0)?))),
            Fresh::Live => Entity::Live(Live::Electro5(ne5::live::new(
                (0, 0).try_into().map_err(|e| format!("{e}"))?,
            ))),
            // A set list is four pointers and nothing else, so the only starting point
            // there is one is the first four programs.
            Fresh::SetList => Entity::Song(Song::Electro5(ne5::song::new(
                (0, 0).try_into().map_err(|e| format!("{e}"))?,
                ne5::song::DEFAULT_VERSION,
                [at(0)?, at(1)?, at(2)?, at(3)?],
            ))),
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
        };
        nord_format::to_bytes(&entity).map_err(|e| e.to_string())
    }
}

/// One asset as a store holds it.
pub struct Saved {
    pub id: u64,
    pub name: String,
    pub origin: Origin,
    pub bytes: Vec<u8>,
}

/// What a background task hands back to the UI thread.
enum Incoming {
    Opened { name: String, bytes: Vec<u8> },
    Note(String),
    Failed(String),
}

/// Which bytes a save writes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ExportWhat {
    File,
    Body,
}

pub struct Workspace {
    entities: Vec<LocalEntity>,
    selected: Option<u64>,
    next_id: u64,
    /// Bumped by every change to the list, so the shell can tell when the store is
    /// behind without comparing every asset's bytes.
    revision: u64,
    ctx: egui::Context,
    tx: Sender<Incoming>,
    rx: Receiver<Incoming>,
}

impl Workspace {
    pub fn new(ctx: egui::Context) -> Workspace {
        let (tx, rx) = std::sync::mpsc::channel();
        Workspace {
            entities: Vec::new(),
            selected: None,
            next_id: 1,
            revision: 0,
            ctx,
            tx,
            rx,
        }
    }

    pub fn selected(&self) -> Option<&LocalEntity> {
        let id = self.selected?;
        self.entities.iter().find(|e| e.id == id)
    }

    /// Point the document view at one entity. The browser's own selection is separate:
    /// clicking a row in the sidebar does not change what the open tab is showing.
    pub fn select(&mut self, id: Option<u64>) {
        self.selected = id;
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

    /// Whether this is a view of a slot rather than something on this computer.
    pub fn is_view(&self, id: u64) -> bool {
        self.get(id).is_some_and(|entity| !entity.kept)
    }

    /// Take a copy of a slot without putting it in the local list.
    ///
    /// What a double-click on a slot opens: a tab and a document over a working copy,
    /// which is edited and sent back like any other, and which goes when its tab does.
    pub fn view(&mut self, name: String, origin: Origin, bytes: Vec<u8>, log: &mut Log) -> u64 {
        let id = self.ingest(name, origin, bytes, log);
        let Some(entity) = self.entities.iter_mut().find(|e| e.id == id) else {
            return id;
        };
        entity.kept = false;
        // ⚠️ Said again, differently. `ingest` has already announced this as being on
        // this computer, which is the one thing a view is not — and the banner over the
        // document says so. The last line the operator reads has to be the true one.
        let where_ = match entity.origin.slot() {
            Some((class, at)) => crate::strings::place(class, at),
            None => "the instrument".to_string(),
        };
        log.say(format!(
            "Viewing {where_} — “{}” is not kept on this computer.",
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

    /// Promote a view into the local list. Its edits and its debt come with it.
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

    /// Drop the views nothing is looking at any more — except the ones holding changes.
    ///
    /// A view lives for as long as its tab: nothing lists it, so a view left behind is
    /// one nothing can reach and nothing can remove.
    ///
    /// ⚠️ **A view is the only copy of what it holds.** Nothing lists it and the store
    /// skips it, so closing its tab is the one gesture in this app that can destroy an
    /// edit — and the × sits beside the badge saying the edit is owed back to a slot.
    /// So an edited or owed view is promoted into the list instead, and only an
    /// untouched one is dropped.
    pub fn close_views(&mut self, open: impl Fn(u64) -> bool, log: &mut Log) {
        let mut rescued = Vec::new();
        let before = self.entities.len();
        self.entities.retain_mut(|entity| {
            if entity.kept || open(entity.id) {
                return true;
            }
            if !precious(entity) {
                return false;
            }
            entity.kept = true;
            rescued.push(entity.name.clone());
            true
        });
        for name in &rescued {
            log.say(format!(
                "“{name}” is kept on this computer — it has changes the instrument does \
                 not."
            ));
        }
        if self.entities.len() == before && rescued.is_empty() {
            return;
        }
        if self.selected.is_some_and(|id| self.get(id).is_none()) {
            self.selected = self.entities.last().map(|e| e.id);
        }
        self.revision += 1;
    }

    /// Mark an asset as owed back to the slot it came from, or paid.
    pub fn mark_pending(&mut self, id: u64, pending: bool) {
        if let Some(entity) = self.entities.iter_mut().find(|e| e.id == id) {
            if entity.pending != pending {
                entity.pending = pending;
                self.revision += 1;
            }
        }
    }

    /// Everything waiting to go back to the instrument, in the order it was opened.
    pub fn pending(&self) -> Vec<&LocalEntity> {
        self.entities.iter().filter(|e| e.pending).collect()
    }

    /// Rename an asset held here. Nothing leaves this computer.
    pub fn rename(&mut self, id: u64, name: String) {
        if let Some(entity) = self.entities.iter_mut().find(|e| e.id == id) {
            entity.name = name;
            self.revision += 1;
        }
    }

    /// Decode `bytes`, badge them, and add the row. Every way in — drop, picker,
    /// fresh default, and later a device read — lands here.
    pub fn ingest(&mut self, name: String, origin: Origin, bytes: Vec<u8>, log: &mut Log) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let entity = LocalEntity::new(id, name, origin, bytes);
        match (&entity.parse_error, &entity.verify) {
            (Some(e), _) => {
                log.error(format!("{}: {e}", entity.name));
                log.trouble(format!(
                    "“{}” is not a file this app understands.",
                    entity.name
                ));
            }
            (None, VerifyState::Ok) => {
                log.info(format!(
                    "{}: {} ({} bytes), verified",
                    entity.name,
                    entity.tag(),
                    entity.bytes.len(),
                ));
                log.say(format!("“{}” is on this computer.", entity.name));
            }
            (None, state) => {
                log.warn(format!(
                    "{}: {} — verify {}: {}",
                    entity.name,
                    entity.tag(),
                    state.badge(),
                    state.detail(),
                ));
                log.say(format!(
                    "“{}” opened, but it does not re-save byte for byte.",
                    entity.name
                ));
            }
        }
        self.entities.push(entity);
        self.selected = Some(id);
        self.revision += 1;
        id
    }

    /// Drain whatever the pickers finished with. Call once per frame.
    pub fn poll(&mut self, log: &mut Log) {
        while let Ok(message) = self.rx.try_recv() {
            match message {
                Incoming::Opened { name, bytes } => {
                    self.ingest(name.clone(), Origin::File(name), bytes, log);
                }
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

    /// The filename an export suggests.
    ///
    /// ⚠️ The one place a name becomes a filename — and the one place a name is made
    /// path-safe. Everywhere else the name is verbatim: what the instrument or the
    /// operator called the thing, spaces and all, and that spelling is what a rename
    /// sends back to the instrument. Sanitising anywhere earlier is how "Big strings"
    /// once came back as "Big-strings".
    pub fn export_name(&self, id: u64, what: ExportWhat) -> Option<String> {
        let entity = self.get(id)?;
        Some(export_filename(&entity.name, &entity.bytes, what))
    }

    pub fn export(&self, id: u64, what: ExportWhat) {
        let Some(entity) = self.entities.iter().find(|e| e.id == id) else {
            return;
        };
        let name = match self.export_name(id, what) {
            Some(name) => name,
            None => return,
        };
        let bytes = match what {
            ExportWhat::File => Some(entity.bytes.clone()),
            ExportWhat::Body => entity.raw_body(),
        };
        let Some(bytes) = bytes else {
            let _ = self.tx.send(Incoming::Failed(format!(
                "{}: not a CBIN container, so there is no header to strip",
                entity.name,
            )));
            return;
        };
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        spawn(async move {
            let _ = tx.send(save(name, bytes).await);
            ctx.request_repaint();
        });
    }

    /// Put back the bytes a tab opened with. The asset is unchanged again, so the dirty
    /// mark goes with them.
    pub fn restore_bytes(&mut self, id: u64, bytes: Vec<u8>, log: &mut Log) {
        let Some(entity) = self.entities.iter_mut().find(|e| e.id == id) else {
            return;
        };
        if entity.bytes == bytes {
            return;
        }
        *entity = LocalEntity {
            kept: entity.kept,
            ..LocalEntity::new(id, entity.name.clone(), entity.origin.clone(), bytes)
        };
        self.revision += 1;
        log.say(format!("“{}” is back as it was opened.", entity.name));
    }

    /// Swap in re-encoded bytes, keeping the entity's identity and marking it edited.
    ///
    /// The decode and the verify are re-run: an editor's output is bytes like any other,
    /// and it earns its badge the same way a file off disk does.
    pub fn replace_bytes(&mut self, id: u64, bytes: Vec<u8>, log: &mut Log) {
        let Some(entity) = self.entities.iter_mut().find(|e| e.id == id) else {
            return;
        };
        let replaced = LocalEntity::new(id, entity.name.clone(), entity.origin.clone(), bytes);
        let verify = replaced.verify.clone();
        *entity = LocalEntity {
            dirty: true,
            pending: entity.pending,
            kept: entity.kept,
            ..replaced
        };
        self.revision += 1;
        if let VerifyState::Ok = verify {
            return;
        }
        log.warn(format!(
            "after editing, verify {}: {}",
            verify.badge(),
            verify.detail()
        ));
    }

    pub fn duplicate(&mut self, id: u64, log: &mut Log) -> Option<u64> {
        let source = self.entities.iter().find(|e| e.id == id)?;
        let name = format!("{} copy", source.name);
        let (origin, bytes) = (source.origin.clone(), source.bytes.clone());
        Some(self.ingest(name, origin, bytes, log))
    }

    pub fn remove(&mut self, id: u64, log: &mut Log) {
        let Some(at) = self.entities.iter().position(|e| e.id == id) else {
            return;
        };
        let gone = self.entities.remove(at);
        self.revision += 1;
        if self.selected == Some(id) {
            self.selected = self.entities.last().map(|e| e.id);
        }
        log.say(format!("Removed “{}” from this computer.", gone.name));
    }

    /// The next id a new asset would take, for the store to carry over.
    pub fn next_id(&self) -> u64 {
        self.next_id
    }

    /// Put back what a previous session held.
    ///
    /// Every asset is decoded and re-checked on the way in: bytes out of a store have
    /// been sitting somewhere this app does not control and get no more trust than bytes
    /// off a disk.
    pub fn restore(&mut self, saved: Vec<Saved>, next_id: Option<u64>, log: &mut Log) {
        for Saved {
            id,
            name,
            origin,
            bytes,
        } in saved
        {
            let entity = LocalEntity::new(id, name, origin, bytes);
            if let Some(e) = &entity.parse_error {
                log.warn(format!("{}: {e}", entity.name));
            }
            self.next_id = self.next_id.max(id + 1);
            self.entities.push(entity);
        }
        if let Some(next) = next_id {
            self.next_id = self.next_id.max(next);
        }
        self.selected = self.entities.last().map(|e| e.id);
        self.revision += 1;
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
        return Incoming::Note(format!("{name}: save cancelled"));
    };
    match handle.write(&bytes).await {
        Ok(()) => Incoming::Note(format!(
            "wrote {} ({} bytes)",
            handle.file_name(),
            bytes.len(),
        )),
        Err(e) => Incoming::Failed(format!("{name}: {e}")),
    }
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
        LocalEntity::new(1, name.into(), Origin::Fresh, bytes)
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
        assert_eq!(container.body_len, ne5::program::BODY_LEN as u64);
        assert_eq!(container.checksum_label, "crc32:");
    }

    /// Each fresh default carries its own tag, and each one round-trips.
    ///
    /// The whole claim the New menu makes about a zeroed body: it decodes, and it
    /// re-saves byte for byte.
    #[test]
    fn every_fresh_default_round_trips_under_its_own_tag() {
        for kind in Fresh::ALL {
            let entity = ingest("untitled", kind.bytes().unwrap());
            assert!(entity.parse_error.is_none(), "{:?}", kind);
            assert_eq!(entity.tag(), kind.tag(), "{kind:?}");
            assert!(matches!(entity.verify, VerifyState::Ok), "{kind:?}");
            assert!(
                entity.container.expect("a CBIN file").checksum_ok,
                "{kind:?}"
            );
        }
    }

    /// The menu is the families, and the families are the menu: a kind reachable from
    /// neither or from two places is one nobody can find or one offered twice.
    #[test]
    fn every_kind_sits_in_exactly_one_family() {
        let mut seen: Vec<Fresh> = Fresh::FAMILIES
            .iter()
            .flat_map(|family| family.kinds.iter().copied())
            .collect();
        assert_eq!(seen.len(), Fresh::ALL.len());
        for kind in Fresh::ALL {
            let at = seen.iter().position(|held| *held == kind);
            seen.remove(at.unwrap_or_else(|| panic!("{kind:?} is in no family")));
        }
        assert!(seen.is_empty());
    }

    /// A zeroed body is not a factory program, and the menu has to say so rather than
    /// let the operator find out.
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
        // It is still an entity in every other way — a tab and a send both find it.
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

    /// A view outlives nothing: once no tab holds it, it is gone. What was kept stays
    /// whether anything is looking at it or not.
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

        workspace.close_views(|id| id == viewed, &mut log);
        assert!(workspace.get(viewed).is_some(), "its tab is still open");

        workspace.close_views(|_| false, &mut log);
        assert!(workspace.get(viewed).is_none());
        assert!(workspace.get(local).is_some(), "kept is kept");
        assert_eq!(workspace.selected().map(|e| e.id), Some(local));
    }

    /// ⚠️ The loss this rule exists to stop. A view is the only copy of what it holds —
    /// nothing lists it and the store skips it — so the × on its tab sits next to a
    /// badge saying the edit is owed to a slot, with no undo behind it. An edited view
    /// is kept; only an untouched one is dropped.
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
        workspace.mark_pending(owed, true);
        assert!(precious(workspace.get(edited).unwrap()));
        assert!(precious(workspace.get(owed).unwrap()));
        assert!(!precious(workspace.get(untouched).unwrap()));

        // Every tab closes at once.
        workspace.close_views(|_| false, &mut log);

        assert!(workspace.get(untouched).is_none(), "the slot still has it");
        let listed: Vec<u64> = workspace.listed().map(|e| e.id).collect();
        assert_eq!(listed, vec![edited, owed], "and the changes survive");
        assert!(!workspace.is_view(edited) && !workspace.is_view(owed));
        // What was owed is still owed: promoting it must not pay a debt.
        assert!(workspace.get(owed).unwrap().pending);
        assert!(log.status().1.contains("kept on this computer"));
    }

    /// One view per slot, so a second double-click has something to be pointed at rather
    /// than a second working copy to make.
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

    /// The raw-body export is the file with its container stripped — for a type-1
    /// file, everything from `body_start` on.
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

    /// The name is verbatim everywhere; only the export dialog sees a path-safe form,
    /// with the extension supplied from the bytes when the name carries none.
    #[test]
    fn an_export_sanitises_the_name_and_supplies_the_extension() {
        let bytes = Fresh::Program.bytes().unwrap();
        let file = |name: &str| export_filename(name, &bytes, ExportWhat::File);
        assert_eq!(file("Big strings"), "Big-strings.ne5p");
        assert_eq!(file("patch.ne5p"), "patch.ne5p", "a carried tag is kept");
        assert_eq!(file("Bass 2.0"), "Bass-2.0.ne5p", "a dot is not a tag");
        assert_eq!(file("../../etc/passwd"), "etc-passwd.ne5p");
        assert_eq!(file("  "), "unnamed.ne5p");
        assert_eq!(
            export_filename("Big strings", b"no header", ExportWhat::File),
            "Big-strings.bin",
        );
        assert_eq!(
            export_filename("Big strings.body", &bytes, ExportWhat::Body),
            "Big-strings.body",
            "a body dump is not double-tagged"
        );
        assert_eq!(
            export_filename("Big strings", &bytes, ExportWhat::Body),
            "Big-strings.body",
        );
    }

    /// A file that does not decode is still a row: the error is the report.
    #[test]
    fn bytes_that_do_not_decode_are_kept_with_their_error() {
        let entity = ingest("junk.bin", b"not a nord file at all".to_vec());
        assert!(entity.entity.is_none());
        assert!(entity.parse_error.is_some());
        assert!(entity.container.is_none());
        assert!(matches!(entity.verify, VerifyState::NotApplicable(_)));
        assert_eq!(entity.tag(), "?");
    }

    /// The name is this app's metadata and the only record of what an object is — a
    /// file stores none. It has to survive the whole way: off the instrument, into a
    /// tab, through an edit, out to a filename.
    #[test]
    fn a_name_survives_being_fetched_opened_edited_and_exported() {
        use nord_usb::{Location, ObjectClass};

        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx);
        let mut log = Log::default();
        let at = Location { bank: 6, slot: 3 };

        // As a read off the instrument arrives: the device supplies the name, nothing
        // else ever does.
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
        assert!(workspace.get(id).unwrap().dirty);

        // Reverting is not renaming either.
        workspace.restore_bytes(id, bytes, &mut log);
        assert_eq!(workspace.get(id).unwrap().name, "Africa-Split.ne5p");

        // And the filename an export offers is that same name, not one worked back out
        // of the bytes.
        assert_eq!(
            workspace.export_name(id, ExportWhat::File).as_deref(),
            Some("Africa-Split.ne5p")
        );
        assert_eq!(
            workspace.export_name(id, ExportWhat::Body).as_deref(),
            Some("Africa-Split.ne5p.body")
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
                bytes: Fresh::Program.bytes().unwrap(),
            }],
            Some(10),
            &mut log,
        );
        let entity = workspace.get(9).expect("restored under its own id");
        assert_eq!(entity.name, "Africa-Split.ne5p");
        assert!(matches!(entity.verify, VerifyState::Ok));
        assert!(!entity.dirty);
        // A new asset cannot land on an id something restored is already using.
        let fresh = workspace.create(Fresh::Live, &mut log).unwrap();
        assert!(fresh >= 10);
    }

    /// A flipped body byte is reported rather than absorbed: the container's own
    /// checksum no longer matches, and the decode says so.
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
