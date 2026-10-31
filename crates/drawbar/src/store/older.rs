//! What a drawbar from before the library was toshokan's kept in a library's folder:
//! tags, unsaved edits and the slot contents it rescued. The tags and origins of its
//! index, `.drawbar/library.ron`, are taken into the library once ([`index`]); the rest
//! is left where it is, and the app tells the user once per library where ([`notice`]).

use std::collections::{BTreeMap, BTreeSet};
use std::io;

use serde::{Deserialize, Serialize};
use toshokan::asynch::Fs;
use toshokan::io::{Kind, Range, Root};
use toshokan::RelPath;

use super::keys::{Came, DIR};
use super::storage;

/// One kind of thing an older drawbar kept, in the folder of drawbar's own data in the
/// library.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Older {
    Tags,
    Edits,
    Rescues,
}

impl Older {
    /// What it is, and where it is under the folder of drawbar's own data.
    pub fn said(self) -> (&'static str, &'static str) {
        match self {
            Older::Tags => ("tags", "library.ron"),
            Older::Edits => ("unsaved edits", "working"),
            Older::Rescues => ("rescued slot contents", "tmp"),
        }
    }
}

fn at(path: &str) -> RelPath {
    RelPath::new(path).expect("a fixed path")
}

/// What an older drawbar kept in the folder.
pub async fn found<F: Fs>(fs: &F) -> io::Result<Vec<Older>> {
    let kind = async |path: &str| -> io::Result<Option<Kind>> {
        Ok(
            storage::meta(fs, Root::Folder, &at(&format!("{DIR}/{path}")))
                .await?
                .map(|meta| meta.kind),
        )
    };
    let mut found = Vec::new();
    let taken_in = index(fs).await?.is_some();
    if kind("library.ron").await? == Some(Kind::File) && !taken_in {
        found.push(Older::Tags);
    }
    if kind("working").await? == Some(Kind::Directory) {
        found.push(Older::Edits);
    }
    if kind("tmp").await? == Some(Kind::Directory) {
        let tmp = storage::list(fs, Root::Folder, &at(&format!("{DIR}/tmp"))).await?;
        if tmp.iter().any(|(name, _)| name.starts_with("rescued-")) {
            found.push(Older::Rescues);
        }
    }
    Ok(found)
}

/// The largest index read: an older drawbar's index held a row only per tagged, edited
/// or slot-born asset.
const MOST: u64 = 64 << 20;

/// What an older drawbar's index says that the library keeps: each file's tags and
/// where it came from, and every tag made.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Index {
    /// The BLAKE3 of the index's bytes, by which the library remembers it took it in.
    pub hash: String,
    pub tags: BTreeSet<String>,
    pub files: BTreeMap<String, (BTreeSet<String>, Option<Came>)>,
}

/// The index an older drawbar kept in the folder, where it is one this build takes in:
/// version 1, the only one written.
pub async fn index<F: Fs>(fs: &F) -> io::Result<Option<Index>> {
    let path = at(&format!("{DIR}/library.ron"));
    let Some(stat) = storage::stat(fs, &path).await? else {
        return Ok(None);
    };
    if stat.len > MOST {
        return Ok(None);
    }
    let range = Range {
        offset: 0,
        len: stat.len,
    };
    let bytes = storage::read(fs, Root::Folder, &path, range).await?;
    Ok(parse(&bytes))
}

/// What the index `bytes` says, where it is one of version 1.
fn parse(bytes: &[u8]) -> Option<Index> {
    let text = std::str::from_utf8(bytes).ok()?;
    let index: V1 = ron::from_str(text).ok()?;
    if index.version != 1 {
        return None;
    }
    let name = |id: &u64| index.tags.get(id).cloned();
    let files = index.assets.into_values().filter_map(|row| {
        let tags: BTreeSet<String> = row.tags.iter().filter_map(name).collect();
        let came = row.origin.came();
        let path = row.path?;
        (!tags.is_empty() || came.is_some()).then_some((path, (tags, came)))
    });
    let files = files.collect();
    Some(Index {
        hash: blake3::hash(bytes).to_hex().to_string(),
        tags: index.tags.into_values().collect(),
        files,
    })
}

/// Version 1 of the index, as the drawbar that wrote it declared it, but for the
/// fields this build does not take in, which it passes over.
#[derive(Deserialize)]
struct V1 {
    version: u32,
    #[serde(default)]
    tags: BTreeMap<u64, String>,
    #[serde(default)]
    assets: BTreeMap<u64, Row>,
}

#[derive(Deserialize)]
struct Row {
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    tags: BTreeSet<u64>,
    #[serde(default)]
    origin: Stored,
}

/// Where an asset came from, as version 1 wrote it: not at all where its file says.
#[derive(Default, Deserialize)]
enum Stored {
    #[default]
    Implied,
    Fresh,
    File(#[expect(dead_code, reason = "a file's own name is no origin the library keeps")] String),
    Device {
        class: u32,
        bank: u32,
        slot: u32,
    },
    Rescued {
        bank: u32,
        slot: u32,
    },
}

/// The codes version 1 wrote for the classes it kept a slot for, which the library keeps
/// as they are ([`super::keys::class_code`]). It wrote any other class as its partition
/// number, which names no class.
const STORED_CLASSES: [u32; 6] = [1, 3, 4, 5, 6, 7];

impl Stored {
    /// What the library keeps of it, where it keeps anything: a file's own name is no
    /// origin, and a slot of a class version 1 did not number is passed over.
    fn came(&self) -> Option<Came> {
        match *self {
            Stored::Implied | Stored::File(_) => None,
            Stored::Fresh => Some(Came::New),
            Stored::Device { class, bank, slot } => STORED_CLASSES
                .contains(&class)
                .then_some(Came::Slot { class, bank, slot }),
            Stored::Rescued { bank, slot } => Some(Came::Rescued { bank, slot }),
        }
    }
}

/// What the user is told of what an older drawbar kept in the folder.
pub fn notice(older: &[Older]) -> String {
    let said: Vec<String> = older
        .iter()
        .map(|older| {
            let (what, at) = older.said();
            format!("{what} in {DIR}/{at}")
        })
        .collect();
    format!(
        "An older drawbar kept {} in this folder. This drawbar does not read them, and \
         leaves them where they are.",
        crate::strings::listed(&said)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An index in the shape the drawbar before the store wrote it, with
    /// `ron::ser::PrettyConfig::default()`: tags by id, a slot by the class numbers
    /// version 1 fixed, and rows with nothing the library keeps.
    const VERSION_ONE: &str = r#"(
    version: 1,
    next_id: 4,
    next_generation: 1,
    tags: {
        1: "Live",
        2: "Unworn",
    },
    assets: {
        1: (
            path: Some("Grand.ne5p"),
            fingerprint: Some((
                len: 165,
                modified: Some(1761400000000000000),
                crc: Some(305419896),
            )),
            tags: [
                1,
            ],
            origin: Device(
                class: 4,
                bank: 1,
                slot: 7,
            ),
        ),
        2: (
            path: Some("Pad.ne5p"),
            origin: File("Pad.ne5p"),
            working: Some((
                generation: 1,
                keeps: Bytes,
            )),
        ),
        3: (
            name: "view",
            origin: Rescued(
                bank: 0,
                slot: 2,
            ),
        ),
    },
)"#;

    #[test]
    fn a_version_one_index_gives_each_files_tags_and_origin() {
        let index = parse(VERSION_ONE.as_bytes()).expect("version 1");
        assert_eq!(index.tags, BTreeSet::from(["Live".into(), "Unworn".into()]));
        let slot = Came::Slot {
            class: super::super::keys::class_code(nord_usb::ObjectClass::Program).unwrap(),
            bank: 1,
            slot: 7,
        };
        let grand = (BTreeSet::from(["Live".to_string()]), Some(slot));
        assert_eq!(
            index.files,
            BTreeMap::from([("Grand.ne5p".to_string(), grand)])
        );
        assert_eq!(
            index.hash,
            blake3::hash(VERSION_ONE.as_bytes()).to_hex().to_string()
        );
    }

    #[test]
    fn an_index_of_another_version_or_none_is_not_taken_in() {
        for text in ["(version: 2, next_id: 1)", "(tags: [])", "not ron"] {
            assert_eq!(parse(text.as_bytes()), None, "{text}");
        }
    }
}
