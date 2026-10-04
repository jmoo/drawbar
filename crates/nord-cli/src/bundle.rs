//! `nord bundle`: Nord Sound Manager bundles, made from the instrument or a folder, and
//! unpacked to a folder.

use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use nord_format::bundle::archive::{copy_member, Directory, DosTime, Entry, Writer};
use nord_format::bundle::manifest::{self, Manifest};
use nord_format::bundle::{Class, Item, Key, Plan};
use nord_format::cbin::Header;
use nord_format::crc::Crc32Stream;
use nord_usb::bundle::Library;
use nord_usb::op as usb_op;
use nord_usb::{Location, ObjectClass};

use crate::device::{explain, explain_walk, open_usb, transact};
use crate::slot::{noun, shown};
use crate::ui::Ui;

/// Write every member of `bundle` under `out`, at its archive path, with its manifest.
/// Refuses a member whose path would land outside `out` and a file already there.
pub fn unpack(ui: &Ui, bundle: &Path, out: Option<PathBuf>) -> Result<(), String> {
    let at = |e: &dyn std::fmt::Display| format!("{}: {e}", bundle.display());
    let out = out.unwrap_or_else(|| bundle.with_extension(""));
    let mut file = std::fs::File::open(bundle).map_err(|e| at(&e))?;
    let directory = Directory::read_from(&mut file).map_err(|e| at(&e))?;
    for member in &directory.members {
        let path = out.join(relative(&member.entry.name)?);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        let named = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
        let mut written = std::fs::File::create_new(&path).map_err(|e| named(&e))?;
        if let Err(e) = copy_member(&mut file, member, &mut written) {
            drop(written);
            let _ = std::fs::remove_file(&path);
            return Err(named(&e));
        }
    }
    ui.note(format!(
        "unpacked {} member(s) of {} into {}",
        directory.members.len(),
        bundle.display(),
        out.display()
    ));
    Ok(())
}

/// The path under the output folder for an archive path, or an error for one that could
/// leave it: an empty, `.` or `..` component, a backslash, or a drive.
fn relative(name: &str) -> Result<PathBuf, String> {
    let unsafe_part =
        |part: &str| part.is_empty() || part == "." || part == ".." || part.contains(['\\', ':']);
    if name.split('/').any(unsafe_part) {
        return Err(format!("{name:?} is not a path inside the bundle"));
    }
    Ok(name.split('/').collect())
}

/// Write the files under `dir` as a bundle at `out`, each at its path under `dir`. A
/// `meta.xml` at the top of `dir` is the manifest as it stands; without one, the
/// manifest lists what each program and set list needs among the members.
pub fn pack(ui: &Ui, dir: &Path, out: &Path) -> Result<(), String> {
    let mut files = Vec::new();
    walk(dir, &mut files)?;
    let meta = dir.join(manifest::PATH);
    let mut members = Vec::new();
    for path in files.iter().filter(|path| **path != meta) {
        let name = archive_name(dir, path)?;
        let header = header_of(path)?;
        let stem = path.file_stem().unwrap_or_default().to_string_lossy();
        let entity = match Class::of(&header) {
            Some(Class::Program | Class::SetList) => {
                Some(nord_format::from_path(path).map_err(|e| format!("{name}: {e}"))?)
            }
            _ => None,
        };
        let mut item = Item::of(&header, &stem, entity.as_ref())
            .ok_or_else(|| format!("{name}: not a file an Electro 5 bundle carries"))?;
        item.path = name;
        members.push((item, path.clone()));
    }
    let plan = Plan::new(
        members.iter().map(|(item, _)| item.clone()).collect(),
        CURRENT_FIRMWARE,
    )
    .map_err(|e| e.to_string())?;
    let manifest = match std::fs::read(&meta) {
        Ok(bytes) => {
            Manifest::parse(&bytes).map_err(|e| format!("{}: {e}", meta.display()))?;
            bytes
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            warn_unmet(ui, &plan);
            plan.manifest.to_bytes()
        }
        Err(e) => return Err(format!("{}: {e}", meta.display())),
    };
    let sources = plan.members.iter().map(|item| {
        let (_, path) = members
            .iter()
            .find(|(m, _)| m.path == item.path)
            .expect("planned");
        Source::File(path.clone(), mtime(path))
    });
    write(out, &plan, sources.collect(), manifest)?;
    ui.note(format!(
        "packed {} member(s) of {} into {}",
        plan.members.len(),
        dir.display(),
        out.display()
    ));
    Ok(())
}

/// The firmware a bundle made without an instrument claims: the only one the specimens
/// come from. Inferred from specimens; not confirmed on hardware.
const CURRENT_FIRMWARE: u32 = 204;

fn walk(dir: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .map(|entry| entry.map(|e| e.path()))
        .collect::<Result<_, _>>()
        .map_err(|e| format!("{}: {e}", dir.display()))?;
    entries.sort();
    for path in entries {
        let hidden = path
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with('.'));
        if hidden {
            continue;
        }
        if path.is_dir() {
            walk(&path, files)?;
        } else {
            files.push(path);
        }
    }
    Ok(())
}

fn archive_name(dir: &Path, path: &Path) -> Result<String, String> {
    let parts: Option<Vec<&str>> = path
        .strip_prefix(dir)
        .map_err(|e| e.to_string())?
        .components()
        .map(|c| c.as_os_str().to_str())
        .collect();
    parts
        .map(|parts| parts.join("/"))
        .ok_or_else(|| format!("{}: a name that is not UTF-8", path.display()))
}

fn header_of(path: &Path) -> Result<Header, String> {
    let mut prefix = Vec::new();
    std::fs::File::open(path)
        .and_then(|f| f.take(0x2c).read_to_end(&mut prefix))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Header::from_prefix(&prefix).map_err(|e| format!("{}: {e}", path.display()))
}

fn mtime(path: &Path) -> DosTime {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|since| DosTime::from_unix(since.as_secs()))
        .unwrap_or_default()
}

fn now() -> DosTime {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|since| DosTime::from_unix(since.as_secs()))
        .unwrap_or_default()
}

fn warn_unmet(ui: &Ui, plan: &Plan) {
    for (path, key) in &plan.unmet {
        let what = match key {
            Key::Piano(id) => format!("piano {id:#010x}"),
            Key::Sample(id) => format!("sample {id:#010x}"),
            Key::Program(bank, slot) => {
                format!("the program at {}", crate::slot::shown_at(*bank, *slot))
            }
        };
        ui.warn(format!(
            "{path} needs {what}, which the bundle does not hold"
        ));
    }
}

/// Where one member's bytes come from.
enum Source {
    Bytes(Vec<u8>),
    File(PathBuf, DosTime),
}

/// Writes `plan`'s members from `sources`, in order, then `manifest`, to `out`.
fn write(out: &Path, plan: &Plan, sources: Vec<Source>, manifest: Vec<u8>) -> Result<(), String> {
    let stamp = now();
    crate::edit::replace_with(out, |file| {
        let failed = |e: &dyn std::fmt::Display| format!("{}: {e}", out.display());
        let mut writer = Writer::new(BufWriter::new(file));
        for (item, source) in plan.members.iter().zip(sources) {
            match source {
                Source::Bytes(bytes) => {
                    let entry = entry(&item.path, &bytes, stamp)?;
                    writer
                        .member(entry, &mut &bytes[..])
                        .map_err(|e| failed(&e))?;
                }
                Source::File(path, modified) => {
                    let (size, crc32) = measure(&path)?;
                    let entry = Entry::new(item.path.clone(), size, crc32, modified);
                    let mut body = std::fs::File::open(&path).map_err(|e| failed(&e))?;
                    writer.member(entry, &mut body).map_err(|e| failed(&e))?;
                }
            }
        }
        let entry = entry(manifest::PATH, &manifest, stamp)?;
        writer
            .member(entry, &mut &manifest[..])
            .map_err(|e| failed(&e))?;
        writer
            .finish(&[])
            .map_err(|e| failed(&e))?
            .flush()
            .map_err(|e| failed(&e))
    })
}

fn entry(name: &str, bytes: &[u8], modified: DosTime) -> Result<Entry, String> {
    let size = u32::try_from(bytes.len()).map_err(|_| format!("{name}: over 4 GiB"))?;
    Ok(Entry::new(
        name.into(),
        size,
        nord_format::crc::crc32(bytes),
        modified,
    ))
}

/// A file's length and CRC-32, read through once.
fn measure(path: &Path) -> Result<(u32, u32), String> {
    let failed = |e: std::io::Error| format!("{}: {e}", path.display());
    let mut file = std::fs::File::open(path).map_err(failed)?;
    let mut crc = Crc32Stream::new();
    let mut len: u64 = 0;
    let mut buf = vec![0; 1 << 16];
    loop {
        let n = file.read(&mut buf).map_err(failed)?;
        if n == 0 {
            break;
        }
        crc.update(&buf[..n]);
        len += n as u64;
    }
    let size = u32::try_from(len).map_err(|_| format!("{}: over 4 GiB", path.display()))?;
    Ok((size, crc.value()))
}

/// One object read off the instrument for a bundle.
struct Fetched {
    item: Item,
    source: Source,
}

/// Read `roots` of `class`, every program a root set list plays, and every piano and
/// sample those programs play, into a bundle at `out`. Read-only.
pub fn get(ui: &Ui, class: ObjectClass, roots: Vec<Location>, out: &Path) -> Result<(), String> {
    let mut device = open_usb()?;
    let firmware = device
        .transport()
        .identity()
        .map(|id| u32::from(id.firmware))
        .map_err(|e| e.to_string())?;
    let roots: Vec<(ObjectClass, Location)> = roots.into_iter().map(|at| (class, at)).collect();
    let closure = transact(
        &mut device,
        format!("{} bundle closure", noun(class)),
        |d| nord_usb::block_on(nord_usb::bundle::closure(d, &roots)),
    )
    .map_err(explain_walk)?;
    for row in &closure.unfound {
        ui.warn(format!(
            "no single {} is named {:?} ({:#010x}); left out",
            row.class.label(),
            row.name.trim_end(),
            row.id
        ));
    }

    let parts = out.with_extension("parts");
    std::fs::create_dir_all(&parts).map_err(|e| format!("{}: {e}", parts.display()))?;
    let result = (|| {
        let mut reads = Vec::new();
        for class in [ObjectClass::SetList, ObjectClass::Program] {
            let objects: Vec<_> = closure
                .objects
                .iter()
                .filter(|(held, ..)| *held == class)
                .collect();
            let slots: Vec<Location> = objects.iter().map(|(_, at, _)| *at).collect();
            for (mut read, (_, _, deps)) in read_small(&mut device, class, &slots)?
                .into_iter()
                .zip(objects)
            {
                read.item.needs = nord_usb::bundle::needs(deps);
                reads.push(read);
            }
        }
        for (n, library) in closure.libraries.iter().enumerate() {
            reads.push(read_library(
                ui,
                &mut device,
                library,
                &parts.join(n.to_string()),
            )?);
        }
        let plan = Plan::new(reads.iter().map(|r| r.item.clone()).collect(), firmware)
            .map_err(|e| e.to_string())?;
        warn_unmet(ui, &plan);
        let mut sources = Vec::new();
        for item in &plan.members {
            let at = reads
                .iter()
                .position(|r| r.item.path == item.path)
                .expect("planned");
            sources.push(reads.swap_remove(at).source);
        }
        let manifest = plan.manifest.to_bytes();
        write(out, &plan, sources, manifest)?;
        ui.note(format!(
            "wrote {} member(s) to {}",
            plan.members.len(),
            out.display()
        ));
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&parts);
    result
}

/// Programs or set lists, in one session.
fn read_small(
    device: &mut nord_usb::Device<nord_usb::transport::UsbTransport>,
    class: ObjectClass,
    slots: &[Location],
) -> Result<Vec<Fetched>, String> {
    if slots.is_empty() {
        return Ok(Vec::new());
    }
    let read = transact(device, format!("{} bundle-read", noun(class)), |d| {
        nord_usb::block_on(d.read(class, async |s| {
            let mut out = Vec::new();
            for &at in slots {
                let info = usb_op::info(s, at).await?;
                out.push((at, info, usb_op::read_program(s, at).await?));
            }
            Ok(out)
        }))
    })
    .map_err(explain_walk)?;
    read.into_iter()
        .map(|(at, info, bytes)| {
            let shown = shown(at);
            let header = Header::from_prefix(&bytes).map_err(|e| format!("{shown}: {e}"))?;
            let entity = nord_format::from_stream(&mut std::io::Cursor::new(&bytes))
                .map_err(|e| format!("{shown}: {e}"))?;
            let item = Item::of(&header, info.name.trim_end(), Some(&entity))
                .ok_or_else(|| format!("{shown}: not an object an Electro 5 bundle carries"))?;
            Ok(Fetched {
                item,
                source: Source::Bytes(bytes),
            })
        })
        .collect()
}

/// A piano or sample, streamed to `path`.
fn read_library(
    ui: &Ui,
    device: &mut nord_usb::Device<nord_usb::transport::UsbTransport>,
    library: &Library,
    path: &Path,
) -> Result<Fetched, String> {
    let (class, at) = (library.class, library.at);
    let mut file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    ui.note(format!(
        "reading {} {:?} from {}",
        class.label(),
        library.name,
        shown(at)
    ));
    let received = transact(
        device,
        format!("{} get {}", noun(class), crate::slot::addr(at)),
        |d| nord_usb::block_on(d.read(class, async |s| usb_op::read_into(s, at, &mut file).await)),
    )
    .map_err(|e| explain(e, at))?;
    let header = header_of(path)?;
    let mut item = Item::of(&header, &library.name, None)
        .ok_or_else(|| format!("{}: not an object an Electro 5 bundle carries", shown(at)))?;
    item.provides = Some(match class {
        ObjectClass::Piano => Key::Piano(library.id),
        _ => Key::Sample(library.id),
    });
    let modified = received
        .info
        .modified
        .and_then(|time| DosTime::from_unix(time.into()))
        .unwrap_or_default();
    Ok(Fetched {
        item,
        source: Source::File(path.to_path_buf(), modified),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_archive_path_that_could_leave_the_folder_is_refused() {
        assert_eq!(
            relative("Program/Bank 7/foo.ne5p").unwrap(),
            PathBuf::from("Program").join("Bank 7").join("foo.ne5p")
        );
        for name in ["../x", "a//b", "/abs", "a/./b", "a\\b", "C:/x", ""] {
            assert!(relative(name).is_err(), "{name:?} was accepted");
        }
    }
}
