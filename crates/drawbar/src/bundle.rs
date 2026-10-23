//! Nord bundles, which drawbar holds only on the way in and out: an import unpacks one
//! into a folder of the library, and an export writes one from a selection.

use std::io;
use std::ops::Range;
use std::sync::Arc;

use nord_format::bundle::archive::{Directory, DosTime, Entry};
#[cfg(target_arch = "wasm32")]
use nord_format::bundle::archive::{Tail, TAIL_MAX};
use nord_format::bundle::{manifest, Class, Item, Key, Plan};
use nord_format::cbin::Header;
use nord_format::{Entity, Program};
use nord_usb::ObjectClass;

use crate::browser::Kind;
use crate::device::DeviceState;
use crate::ondisk::OnDisk;
use crate::store::Outside;
use crate::workspace::{Bytes, LocalEntity, VerifyState, Workspace};

/// The extensions of the bundles Nord Sound Manager writes for an Electro 5.
const EXTENSIONS: [&str; 2] = ["ne5pbundle", "ne5tbundle"];

/// Whether a file of this name is a bundle to import.
pub fn is_bundle(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(_, extension)| {
        EXTENSIONS
            .iter()
            .any(|known| extension.eq_ignore_ascii_case(known))
    })
}

/// One file a bundle holds: its archive path, where its bytes lie in the bundle, and
/// their CRC-32.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    pub path: String,
    pub bytes: Range<u64>,
    pub crc32: u32,
}

impl Member {
    /// The member's own name, the last part of its archive path.
    pub fn leaf(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }
}

/// The files a bundle holds, its manifest left out, read from its directory without
/// reading any member.
pub async fn members(from: &Outside) -> io::Result<Vec<Member>> {
    let directory = directory(from).await?;
    Ok(directory
        .members
        .into_iter()
        .filter(|member| member.entry.name != manifest::PATH)
        .map(|member| Member {
            path: member.entry.name,
            bytes: member.body,
            crc32: member.entry.crc32,
        })
        .collect())
}

fn invalid(e: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e.to_string())
}

#[cfg(not(target_arch = "wasm32"))]
async fn directory(from: &Outside) -> io::Result<Directory> {
    let mut file = std::fs::File::open(from)?;
    Directory::read_from(&mut file).map_err(invalid)
}

#[cfg(target_arch = "wasm32")]
async fn directory(from: &Outside) -> io::Result<Directory> {
    use crate::ondisk::slice;
    let len = from.size() as u64;
    let tail = slice(from, len - len.min(TAIL_MAX)..len).await?;
    let tail = Tail::find(&tail, len).map_err(invalid)?;
    let bytes = slice(from, tail.directory.clone()).await?;
    let mut directory = tail.directory(&bytes).map_err(invalid)?;
    for member in &mut directory.members {
        let header = slice(from, member.header.clone()).await?;
        member.check_header(&header).map_err(invalid)?;
    }
    Ok(directory)
}

/// Where an exported member's bytes are.
pub enum Body {
    Held(Bytes),
    Resting(Arc<OnDisk>),
}

/// A bundle laid out from assets on this computer, ready to write.
pub struct Export {
    /// The file name it is offered under.
    pub name: String,
    pub plan: Plan,
    /// One per member of the plan, in its order.
    pub bodies: Vec<Body>,
    /// Assets left out, each with why.
    pub left_out: Vec<String>,
}

/// What laying out a bundle came to.
pub enum Laid {
    Ready(Export),
    /// These assets, checked or perhaps needed, must be read first.
    Read(Vec<u64>),
}

/// Lay out a bundle of `ids`, with every piano, sample and program they need that this
/// computer holds.
///
/// A program names a piano or sample by an id its file does not hold, so one is found
/// by the name the instrument gave that id. A set list names programs by slot, so one is
/// found where exactly one program on this computer claims that slot.
pub fn lay_out(ids: &[u64], workspace: &Workspace, device: &DeviceState) -> Result<Laid, String> {
    let mut chosen: Vec<u64> = Vec::new();
    let mut members: Vec<(Item, Body)> = Vec::new();
    let mut left_out = Vec::new();
    let mut to_read: Vec<u64> = Vec::new();
    let mut adding: Vec<u64> = ids.to_vec();
    while !adding.is_empty() {
        for id in adding.drain(..) {
            if chosen.contains(&id) {
                continue;
            }
            chosen.push(id);
            let Some(entity) = workspace.get(id) else {
                continue;
            };
            if entity.unread() || entity.reading() {
                to_read.push(id);
                continue;
            }
            match member(entity, device) {
                Ok(member) => members.push(member),
                Err(why) => left_out.push(why),
            }
        }
        let provided = |need: &Key| members.iter().any(|(m, _)| m.provides == Some(*need));
        let unmet: Vec<Key> = members
            .iter()
            .flat_map(|(m, _)| m.needs.iter().copied())
            .filter(|need| !provided(need))
            .collect();
        for need in unmet {
            match provider(need, workspace, device) {
                Provider::One(id) if !chosen.contains(&id) => adding.push(id),
                Provider::Many => {
                    let why = format!("more than one file on this computer is {}", needed(need));
                    if !left_out.contains(&why) {
                        left_out.push(why);
                    }
                }
                Provider::Unread(ids) => to_read.extend(ids),
                Provider::One(_) | Provider::None => {}
            }
        }
    }
    if !to_read.is_empty() {
        to_read.sort_unstable();
        to_read.dedup();
        return Ok(Laid::Read(to_read));
    }
    let firmware = device
        .card()
        .and_then(|card| card.firmware)
        .map_or(manifest::ELECTRO5_FIRMWARE, u32::from);
    let items = members.iter().map(|(item, _)| item.clone()).collect();
    let plan = Plan::new(items, firmware).map_err(|e| e.to_string())?;
    let mut bodies = Vec::with_capacity(plan.members.len());
    for item in &plan.members {
        let at = members
            .iter()
            .position(|(m, _)| m.path == item.path)
            .expect("each planned member was laid out");
        bodies.push(members.swap_remove(at).1);
    }
    let root = plan
        .members
        .iter()
        .rev()
        .find(|m| matches!(m.class, Class::SetList | Class::Program))
        .or(plan.members.first())
        .ok_or("nothing checked is a file a bundle carries")?;
    let extension = match root.class {
        Class::SetList => "ne5tbundle",
        _ => "ne5pbundle",
    };
    let stem = root.path.rsplit('/').next().unwrap_or_default();
    let stem = stem.rsplit_once('.').map_or(stem, |(stem, _)| stem);
    Ok(Laid::Ready(Export {
        name: crate::store::names::portable(&format!("{stem}.{extension}")),
        plan,
        bodies,
        left_out,
    }))
}

/// Whether a bundle may carry an asset of this kind. [`lay_out`] decides whether it
/// carries the instrument's file the asset is.
pub fn carries(kind: Kind) -> bool {
    matches!(
        kind,
        Kind::Program | Kind::SetList | Kind::Piano | Kind::Sample
    )
}

/// The member an asset becomes, or why it is left out.
fn member(entity: &LocalEntity, device: &DeviceState) -> Result<(Item, Body), String> {
    let carried = || format!("“{}” is not a file a bundle carries", entity.name);
    let (header, body) = match entity.rests() {
        Some(file) => (file.index.header().clone(), Body::Resting(file.clone())),
        None => (
            Header::from_prefix(&entity.bytes).map_err(|_| carried())?,
            Body::Held(entity.bytes.clone()),
        ),
    };
    // What a program or set list needs is in its body, so one not decoded yet is
    // decoded here.
    let owned: Entity;
    let decoded = match (Class::of(&header), entity.entity.as_deref()) {
        (Some(Class::Program | Class::SetList), Some(decoded)) => Some(decoded),
        (Some(Class::Program | Class::SetList), None) => {
            owned = nord_format::from_stream(&mut io::Cursor::new(&entity.bytes[..]))
                .map_err(|e| format!("“{}” does not decode: {e}", entity.name))?;
            Some(&owned)
        }
        _ => None,
    };
    let name = stem(entity);
    let mut item = Item::of(&header, &name, decoded).ok_or_else(carried)?;
    // The instrument's own list of what the slot an object came from needs counts over
    // the file's: it can name a piano or sample the file leaves at zero. Confirmed on
    // hardware.
    if let Some((class, at)) = entity.origin.slot() {
        if let Some(needs) = device.needs_of(class, at) {
            item.needs = needs.to_vec();
        }
    }
    item.provides = item.provides.or_else(|| match item.class {
        Class::Piano => device.library_id(ObjectClass::Piano, &name).map(Key::Piano),
        Class::Sample => device
            .library_id(ObjectClass::Sample, &name)
            .map(Key::Sample),
        Class::Program | Class::SetList => None,
    });
    Ok((item, body))
}

/// What a need names, in words.
pub fn needed(need: Key) -> String {
    match need {
        Key::Piano(id) => format!("the piano the instrument knows as {id:#010x}"),
        Key::Sample(id) => format!("the sample the instrument knows as {id:#010x}"),
        Key::Program(bank, slot) => format!(
            "the program at {}",
            crate::strings::shown(nord_usb::Location {
                bank: bank.into(),
                slot: slot.into()
            },)
        ),
    }
}

/// An asset's name without the extension its file carries.
fn stem(entity: &LocalEntity) -> String {
    let name = entity.name.trim();
    match name.rsplit_once('.') {
        Some((stem, extension))
            if crate::browser::tagged(name).is_some() && !extension.is_empty() =>
        {
            stem.to_string()
        }
        _ => name.to_string(),
    }
}

/// Who on this computer provides a need.
enum Provider {
    One(u64),
    /// More than one asset does, so none is picked.
    Many,
    /// None that has been read does, and these programs are still to be read.
    Unread(Vec<u64>),
    None,
}

/// The asset on this computer that provides `need`.
fn provider(need: Key, workspace: &Workspace, device: &DeviceState) -> Provider {
    let found: Vec<u64> = match need {
        Key::Piano(id) | Key::Sample(id) => {
            let (class, tag) = match need {
                Key::Piano(_) => (ObjectClass::Piano, "npno"),
                _ => (ObjectClass::Sample, "nsmp"),
            };
            let Some(name) = device.dependency_name(class, id) else {
                return Provider::None;
            };
            let name = name.trim();
            workspace
                .entities()
                .iter()
                .filter(|e| stem(e) == name && e.format_tag() == tag)
                .map(|e| e.id)
                .collect()
        }
        Key::Program(bank, slot) => workspace
            .entities()
            .iter()
            .filter(|e| match e.entity.as_deref() {
                Some(Entity::Program(Program::Electro5(file))) => {
                    file.header.slot() == (bank, slot)
                }
                _ => false,
            })
            .map(|e| e.id)
            .collect(),
    };
    // A program not read yet may claim the slot too, unless it could not be read.
    let unread: Vec<u64> = match need {
        Key::Program(..) => workspace
            .entities()
            .iter()
            .filter(|e| e.unread() || e.reading())
            .filter(|e| e.format_tag() == "ne5p" && !matches!(e.verify(), VerifyState::NotRead(_)))
            .map(|e| e.id)
            .collect(),
        Key::Piano(_) | Key::Sample(_) => Vec::new(),
    };
    match (found.as_slice(), unread.is_empty()) {
        (_, false) => Provider::Unread(unread),
        ([one], true) => Provider::One(*one),
        ([], true) => Provider::None,
        (_, true) => Provider::Many,
    }
}

/// The entries of `export`'s members, then its manifest's, each stamped `modified`.
async fn entries(export: &Export, manifest: &[u8], modified: DosTime) -> io::Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for (item, body) in export.plan.members.iter().zip(&export.bodies) {
        let (len, crc) = match body {
            Body::Held(bytes) => (bytes.len() as u64, nord_format::crc::crc32(bytes)),
            Body::Resting(file) => (file.len, file.crc_now().await?),
        };
        entries.push(entry(&item.path, len, crc, modified)?);
    }
    let crc = nord_format::crc::crc32(manifest);
    entries.push(entry(manifest::PATH, manifest.len() as u64, crc, modified)?);
    Ok(entries)
}

fn entry(path: &str, len: u64, crc: u32, modified: DosTime) -> io::Result<Entry> {
    let size = u32::try_from(len).map_err(|_| invalid(format!("{path} is over 4 GiB")))?;
    Ok(Entry::new(path.to_string(), size, crc, modified))
}

fn now() -> DosTime {
    crate::work::unix_seconds()
        .and_then(|seconds| DosTime::from_unix(seconds.into()))
        .unwrap_or_default()
}

/// Ask where to save `export`, and write it there. Returns what to say once it is
/// written.
#[cfg(not(target_arch = "wasm32"))]
pub async fn write(export: Export) -> io::Result<String> {
    let Some(handle) = rfd::AsyncFileDialog::new()
        .set_file_name(&export.name)
        .save_file()
        .await
    else {
        return Ok(format!("{}: save canceled", export.name));
    };
    write_to(&export, handle.path()).await?;
    Ok(format!("wrote {}", handle.file_name()))
}

/// Write `export` to `path`, each member streamed from where it is, through a sibling
/// file renamed over `path` once it is whole.
#[cfg(not(target_arch = "wasm32"))]
pub async fn write_to(export: &Export, path: &std::path::Path) -> io::Result<()> {
    use nord_format::bundle::archive::Writer;
    use std::io::Write as _;

    let manifest = export.plan.manifest.to_bytes();
    let entries = entries(export, &manifest, now()).await?;
    let mut temp = path.as_os_str().to_owned();
    temp.push(".tmp");
    let temp = std::path::PathBuf::from(temp);
    let wrote = (|| -> io::Result<()> {
        let mut writer = Writer::new(io::BufWriter::new(std::fs::File::create(&temp)?));
        let mut entries = entries.into_iter();
        for body in &export.bodies {
            let entry = entries.next().expect("an entry per member");
            match body {
                Body::Held(bytes) => writer.member(entry, &mut &bytes[..]),
                Body::Resting(file) => writer.member(entry, &mut file.reader()),
            }
            .map_err(invalid)?;
        }
        let entry = entries.next().expect("an entry for the manifest");
        writer.member(entry, &mut &manifest[..]).map_err(invalid)?;
        writer.finish(&[]).map_err(invalid)?.flush()?;
        std::fs::rename(&temp, path)
    })();
    if wrote.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    wrote
}

/// Hand `export` to the browser's downloads as one file assembled from parts: each
/// member a file the library rests in or bytes this tab holds, between the headers and
/// directory written here, so no member passes through this tab whole.
#[cfg(target_arch = "wasm32")]
pub async fn write(export: Export) -> io::Result<String> {
    use nord_format::bundle::archive::Frame;

    let manifest = export.plan.manifest.to_bytes();
    let entries = entries(&export, &manifest, now()).await?;
    let frame = Frame::new(&entries, &[]).map_err(invalid)?;
    let parts = js_sys::Array::new();
    let bytes = |bytes: &[u8]| js_sys::Uint8Array::from(bytes);
    for (header, body) in frame.headers.iter().zip(&export.bodies) {
        parts.push(&bytes(header));
        match body {
            Body::Held(held) => parts.push(&bytes(held)),
            Body::Resting(file) => {
                let file = file
                    .snapshot()
                    .ok_or_else(|| io::Error::other("drawbar no longer reads a member's file"))?;
                parts.push(&file)
            }
        };
    }
    parts.push(&bytes(frame.headers.last().expect("the manifest's header")));
    parts.push(&bytes(&manifest));
    parts.push(&bytes(&frame.trailer));
    let blob = web_sys::Blob::new_with_u8_array_sequence(&parts)
        .map_err(|e| io::Error::other(format!("{e:?}")))?;
    crate::workspace::hand_over(&export.name, &blob)
        .map_err(|e| io::Error::other(format!("{e:?}")))?;
    Ok(format!("downloaded {}", export.name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bundle_is_named_by_its_extension() {
        assert!(is_bundle("B3 Split.ne5tbundle"));
        assert!(is_bundle("foo.NE5PBUNDLE"));
        assert!(!is_bundle("foo.ne5p"));
        assert!(!is_bundle("ne5pbundle"));
    }
}
