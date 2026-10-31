//! The list of This computer that drawbar 0.10 and earlier kept in eframe's string
//! store, local storage in a browser and `app.ron` on the desktop, which a library takes
//! in once ([`super::Store::take_in`]). Each key held a version line and then one line
//! per row, tab-separated, as that build's `store.rs`, `named.rs`, `folders.rs` and
//! `tags.rs` wrote them.

use std::collections::{BTreeMap, BTreeSet};

use base64::prelude::{Engine as _, BASE64_STANDARD};

use super::keys::Came;

/// The keys the list was kept under: its assets, its folders and its tags.
const KEYS: [&str; 3] = ["drawbar.this_computer", "drawbar.folders", "drawbar.tags"];

/// How many fields an asset's line holds, which is all its version decides.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// `drawbar 1`: id, origin, name, and the bytes, saved and current alike.
    Four,
    /// `drawbar 2`: those, then the current bytes where an edit was unsaved.
    Five,
}

const SHAPES: [(&str, Shape); 2] = [("drawbar 1", Shape::Four), ("drawbar 2", Shape::Five)];

/// The version line and the marker of a named row, of the folders and of the tags. A
/// membership row is marked `m` in both.
const FOLDERS: (&str, &str) = ("drawbar folders 1", "f");
const TAGS: (&str, &str) = ("drawbar tags 1", "t");

/// The classes whose slot an origin kept, by the codes drawbar 0.10 wrote, which are the
/// codes the library keeps ([`super::keys::class_code`]). It wrote any other class as its
/// partition number, which names no class.
const SLOT_CLASSES: [u32; 6] = [1, 3, 4, 5, 6, 7];

/// What eframe's store holds of the list.
#[derive(Debug, PartialEq)]
pub enum Found {
    /// Nothing: every key is absent or empty.
    Nothing,
    /// A key opens with a version line this build does not read, so none is read.
    Unknown,
    List(List),
}

/// The list as it was kept.
#[derive(Debug, Default, PartialEq)]
pub struct List {
    /// The BLAKE3 of the keys' text, by which a library remembers it took the list in.
    pub hash: String,
    pub assets: Vec<Asset>,
    /// Every folder, in the order kept. An asset names its folder by its place here.
    pub folders: Vec<String>,
    /// Every tag made, worn or not.
    pub tags: BTreeSet<String>,
    /// How many lines did not read.
    pub unreadable: usize,
}

/// One asset of the list.
#[derive(Debug, PartialEq)]
pub struct Asset {
    pub name: String,
    /// Where it came from, where that is an origin a library keeps.
    pub came: Option<Came>,
    /// What it was last saved as.
    pub saved: Vec<u8>,
    /// What it held over `saved`, where an edit was unsaved.
    pub unsaved: Option<Vec<u8>>,
    pub folder: Option<usize>,
    pub tags: BTreeSet<String>,
}

impl List {
    /// Whether it holds nothing to take in.
    pub fn is_empty(&self) -> bool {
        self.assets.is_empty() && self.folders.is_empty() && self.tags.is_empty()
    }

    /// What the user is told once a library took it in.
    pub fn said(&self) -> String {
        match self.assets.len() {
            0 => "drawbar brought the tags and folders an earlier version kept into the \
                  library."
                .to_string(),
            n => format!(
                "drawbar brought {} an earlier version kept into the library, with {} tags \
                 and folders.",
                crate::strings::counted(n, "sound", "sounds"),
                if n == 1 { "its" } else { "their" }
            ),
        }
    }
}

/// What the user is told where a key holds a version this build does not read.
pub const UNKNOWN: &str = "An earlier drawbar kept sounds on this computer in a form this \
     version does not read, so it leaves them as they are.";

/// What the user is told of `n` lines of the list that do not read.
pub fn unreadable(n: usize) -> String {
    format!(
        "{} of what an earlier drawbar kept on this computer did not read, so drawbar \
         leaves {} out.",
        crate::strings::counted(n, "line", "lines"),
        if n == 1 { "it" } else { "them" }
    )
}

/// The list eframe's store holds.
pub fn found(storage: &dyn eframe::Storage) -> Found {
    let texts = KEYS.map(|key| storage.get_string(key).unwrap_or_default());
    if texts.iter().all(String::is_empty) {
        return Found::Nothing;
    }
    let [assets, folders, tags] = &texts;
    let mut unreadable = 0;
    let assets = rows(assets, &mut unreadable);
    let folders = grouping(folders, FOLDERS, &mut unreadable);
    let tags = grouping(tags, TAGS, &mut unreadable);
    let (Some(assets), Some(folders), Some(tags)) = (assets, folders, tags) else {
        return Found::Unknown;
    };
    let mut hash = blake3::Hasher::new();
    for text in &texts {
        hash.update(&(text.len() as u64).to_le_bytes());
        hash.update(text.as_bytes());
    }
    let at: BTreeMap<u64, usize> = folders
        .named
        .iter()
        .enumerate()
        .map(|(at, (id, _))| (*id, at))
        .collect();
    // A later membership moves the asset, as it did when the list was read back.
    let held: BTreeMap<u64, u64> = folders.members.iter().copied().collect();
    let named: BTreeMap<u64, &String> = tags.named.iter().map(|(id, name)| (*id, name)).collect();
    let assets = assets.into_iter().map(|(id, asset)| {
        let worn = tags.members.iter().filter(|(asset, _)| *asset == id);
        Asset {
            folder: held.get(&id).and_then(|folder| at.get(folder).copied()),
            tags: worn
                .filter_map(|(_, tag)| named.get(tag))
                .map(|name| (*name).clone())
                .collect(),
            ..asset
        }
    });
    Found::List(List {
        hash: hash.finalize().to_hex().to_string(),
        assets: assets.collect(),
        folders: folders.named.into_iter().map(|(_, name)| name).collect(),
        tags: tags.named.into_iter().map(|(_, name)| name).collect(),
        unreadable,
    })
}

/// Empty the keys the list was kept under.
///
/// ⚠️ eframe's store has no remove, and on the desktop it writes back whatever it holds
/// in memory, so a key is emptied rather than left alone.
pub fn empty(storage: &mut dyn eframe::Storage) {
    for key in KEYS {
        if storage.get_string(key).is_some_and(|held| !held.is_empty()) {
            storage.set_string(key, String::new());
        }
    }
}

/// The assets `text` keeps, each by the id its folder and tags name it by, in the order
/// kept, counting into `unreadable` each line that does not read. `None` for a version
/// this build does not read.
fn rows(text: &str, unreadable: &mut usize) -> Option<Vec<(u64, Asset)>> {
    let mut lines = text.lines();
    let Some(version) = lines.next() else {
        return Some(Vec::new());
    };
    let (_, shape) = SHAPES.into_iter().find(|(line, _)| *line == version)?;
    // The id the next asset would have taken.
    lines.next();
    let mut assets: Vec<(u64, Asset)> = Vec::new();
    for line in lines {
        match row(line, shape) {
            Some((id, asset)) if assets.iter().all(|(held, _)| *held != id) => {
                assets.push((id, asset));
            }
            _ => *unreadable += 1,
        }
    }
    Some(assets)
}

/// One asset's line: its id, origin, escaped name and base64 saved bytes, and in
/// [`Shape::Five`] its base64 current bytes where they differ.
fn row(line: &str, shape: Shape) -> Option<(u64, Asset)> {
    let mut fields = line.splitn(5, '\t');
    let id = fields.next()?.parse().ok()?;
    let came = origin(fields.next()?)?;
    let name = unescape(fields.next()?);
    let saved = BASE64_STANDARD.decode(fields.next()?).ok()?;
    let unsaved = match (shape, fields.next()) {
        (_, None) => None,
        (Shape::Five, Some(text)) => Some(BASE64_STANDARD.decode(text).ok()?),
        (Shape::Four, Some(_)) => return None,
    };
    let asset = Asset {
        name,
        came,
        saved,
        unsaved,
        folder: None,
        tags: BTreeSet::new(),
    };
    Some((id, asset))
}

/// What the library keeps of an origin as drawbar 0.10 wrote it: `fresh`, `file:<name>`,
/// `device:<class>:<bank>:<slot>` or `rescued:<bank>:<slot>`. `None` for one that does
/// not read; a file's name, and a slot of a class [`SLOT_CLASSES`] does not hold, are
/// read and kept as no origin.
fn origin(text: &str) -> Option<Option<Came>> {
    let (head, rest) = text.split_once(':').unwrap_or((text, ""));
    let mut numbers = rest.split(':').map(str::parse::<u32>);
    let mut number = || numbers.next()?.ok();
    Some(match head {
        "fresh" => Some(Came::New),
        "file" => None,
        "device" => {
            let class = number()?;
            let (bank, slot) = (number()?, number()?);
            SLOT_CLASSES
                .contains(&class)
                .then_some(Came::Slot { class, bank, slot })
        }
        "rescued" => {
            let (bank, slot) = (number()?, number()?);
            Some(Came::Rescued { bank, slot })
        }
        _ => return None,
    })
}

/// A grouping's rows: each named folder or tag by its id, in the order kept, and each
/// membership, an asset's id and a group's.
#[derive(Default)]
struct Grouping {
    named: Vec<(u64, String)>,
    members: Vec<(u64, u64)>,
}

/// The rows of the folders or the tags `text` keeps, counting into `unreadable` each line
/// that does not read and each second row of one id. `None` for a version this build
/// does not read.
fn grouping(text: &str, (version, kind): (&str, &str), unreadable: &mut usize) -> Option<Grouping> {
    let mut lines = text.lines();
    let mut grouping = Grouping::default();
    match lines.next() {
        None => return Some(grouping),
        Some(line) if line == version => {}
        Some(_) => return None,
    }
    for line in lines {
        let fields: Vec<&str> = line.split('\t').collect();
        let read = match fields[..] {
            ["m", asset, group] => asset
                .parse()
                .ok()
                .zip(group.parse().ok())
                .map(|member| grouping.members.push(member)),
            [head, id, name] if head == kind => id
                .parse()
                .ok()
                .filter(|id| grouping.named.iter().all(|(held, _)| held != id))
                .map(|id| grouping.named.push((id, unescape(name)))),
            _ => None,
        };
        if read.is_none() {
            *unreadable += 1;
        }
    }
    Some(grouping)
}

/// A name with `\t`, `\n` and `\\` read back as the tab, newline and backslash they
/// escape.
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        let escaped = match (c, chars.clone().next()) {
            ('\\', Some('t')) => '\t',
            ('\\', Some('n')) => '\n',
            ('\\', Some('\\')) => '\\',
            _ => {
                out.push(c);
                continue;
            }
        };
        chars.next();
        out.push(escaped);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Fake;

    fn stored(keys: [&str; 3]) -> Fake {
        let mut storage = Fake::default();
        for (key, text) in KEYS.into_iter().zip(keys) {
            eframe::Storage::set_string(&mut storage, key, text.to_string());
        }
        storage
    }

    fn list(keys: [&str; 3]) -> List {
        match found(&stored(keys)) {
            Found::List(list) => list,
            other => panic!("{other:?}"),
        }
    }

    /// Lines as drawbar 0.10's `store::save` and `named::written` wrote them: `AAEC` is
    /// the base64 of 00 01 02, and `AwQ=` of 03 04.
    #[test]
    fn each_asset_comes_with_its_origin_bytes_folder_and_tags() {
        let list = list([
            "drawbar 2\n4\n\
             1\tdevice:4:1:7\tGrand\\tPiano\tAAEC\n\
             2\tfresh\tPad\tAAEC\tAwQ=\n\
             3\tfile:Lead.ne5p\tLead.ne5p\tAwQ=\n",
            "drawbar folders 1\nf\t1\tGig\nf\t2\tEmpty\nm\t2\t1\n",
            "drawbar tags 1\nt\t1\tLive\nt\t2\tUnworn\nm\t1\t1\nm\t2\t1\n",
        ]);
        let slot = Came::Slot {
            class: super::super::keys::class_code(nord_usb::ObjectClass::Program).unwrap(),
            bank: 1,
            slot: 7,
        };
        let live = BTreeSet::from(["Live".to_string()]);
        assert_eq!(
            list.assets,
            [
                Asset {
                    name: "Grand\tPiano".into(),
                    came: Some(slot),
                    saved: vec![0, 1, 2],
                    unsaved: None,
                    folder: None,
                    tags: live.clone(),
                },
                Asset {
                    name: "Pad".into(),
                    came: Some(Came::New),
                    saved: vec![0, 1, 2],
                    unsaved: Some(vec![3, 4]),
                    folder: Some(0),
                    tags: live,
                },
                Asset {
                    name: "Lead.ne5p".into(),
                    came: None,
                    saved: vec![3, 4],
                    unsaved: None,
                    folder: None,
                    tags: BTreeSet::new(),
                },
            ]
        );
        assert_eq!(list.folders, ["Gig", "Empty"]);
        assert_eq!(list.tags, BTreeSet::from(["Live".into(), "Unworn".into()]));
        assert_eq!(list.unreadable, 0);
    }

    /// Version 1 kept one set of bytes, the saved and the current alike.
    #[test]
    fn a_version_one_line_has_no_unsaved_edit_and_no_fifth_field() {
        let list = list([
            "drawbar 1\n3\n1\tfresh\tA\tAAEC\n2\tfresh\tB\tAAEC\tAwQ=\n",
            "",
            "",
        ]);
        let names: Vec<(&str, bool)> = list
            .assets
            .iter()
            .map(|asset| (asset.name.as_str(), asset.unsaved.is_some()))
            .collect();
        assert_eq!(names, [("A", false)]);
        assert_eq!(list.unreadable, 1, "a fifth field is no version 1 line");
    }

    #[test]
    fn a_line_that_does_not_read_is_counted_and_the_rest_are_read() {
        let list = list([
            "drawbar 2\n9\n\
             x\tfresh\tNo id\tAAEC\n\
             1\tsomewhere\tNo origin\tAAEC\n\
             2\tfresh\tNot base64\t!!\n\
             3\tdevice:4:1\tShort slot\tAAEC\n\
             4\tfresh\tKept\tAAEC\n\
             4\tfresh\tSecond of one id\tAAEC\n\
             \n",
            "drawbar folders 1\nf\tx\tNo id\nm\t4\nf\t1\tGig\nf\t1\tSecond of one id\n",
            "drawbar tags 1\nq\t1\tUnknown marker\nt\t1\tLive\tExtra\nt\t2\tLive\n",
        ]);
        let names: Vec<&str> = list
            .assets
            .iter()
            .map(|asset| asset.name.as_str())
            .collect();
        assert_eq!(names, ["Kept"]);
        assert_eq!(list.folders, ["Gig"]);
        assert_eq!(list.tags, BTreeSet::from(["Live".into()]));
        assert_eq!(list.unreadable, 6 + 3 + 2);
    }

    /// drawbar 0.10 numbered a class it did not name by its partition, and a file's name
    /// is no origin the library keeps.
    #[test]
    fn only_an_origin_the_library_keeps_is_taken() {
        let list = list([
            "drawbar 2\n4\n\
             1\tdevice:2:0:3\tNative\tAAEC\n\
             2\tdevice:8:0:3\tUnnamed\tAAEC\n\
             3\trescued:2:5\tRescued\tAAEC\n\
             4\tfile:x.ne5p\tFile\tAAEC\n",
            "",
            "",
        ]);
        let came: Vec<Option<Came>> = list.assets.iter().map(|asset| asset.came).collect();
        let rescued = Came::Rescued { bank: 2, slot: 5 };
        assert_eq!(came, [None, None, Some(rescued), None]);
    }

    #[test]
    fn a_version_this_build_does_not_read_leaves_the_whole_list_unread() {
        for keys in [
            ["drawbar 3\n1\n", "", ""],
            ["drawbar 2\n1\n", "drawbar folders 2\n", ""],
            ["", "", "tags\nt\t1\tLive\n"],
        ] {
            assert_eq!(found(&stored(keys)), Found::Unknown, "{keys:?}");
        }
    }

    #[test]
    fn keys_absent_or_empty_hold_nothing_and_version_lines_alone_an_empty_list() {
        assert_eq!(found(&Fake::default()), Found::Nothing);
        assert_eq!(found(&stored(["", "", ""])), Found::Nothing);
        let list = list(["drawbar 2\n1\n", "drawbar folders 1\n", "drawbar tags 1\n"]);
        assert!(list.is_empty());
    }

    #[test]
    fn the_hash_changes_with_any_key_and_with_where_a_line_is_kept() {
        let hash = |keys| list(keys).hash;
        let first = hash(["drawbar 2\n1\n", "drawbar folders 1\n", "drawbar tags 1\n"]);
        assert_eq!(
            first,
            hash(["drawbar 2\n1\n", "drawbar folders 1\n", "drawbar tags 1\n"])
        );
        assert_ne!(
            first,
            hash(["drawbar 2\n2\n", "drawbar folders 1\n", "drawbar tags 1\n"])
        );
        assert_ne!(
            hash([
                "drawbar 2\n1\n",
                "drawbar folders 1\nf\t1\tA\n",
                "drawbar tags 1\n"
            ]),
            hash([
                "drawbar 2\n1\nf\t1\tA\n",
                "drawbar folders 1\n",
                "drawbar tags 1\n"
            ]),
        );
    }

    #[test]
    fn emptying_the_keys_leaves_every_other_key() {
        let mut storage = stored(["drawbar 2\n1\n", "drawbar folders 1\n", ""]);
        eframe::Storage::set_string(&mut storage, "drawbar.theme", "dark".into());
        empty(&mut storage);
        assert_eq!(found(&storage), Found::Nothing);
        let theme = eframe::Storage::get_string(&storage, "drawbar.theme");
        assert_eq!(theme.as_deref(), Some("dark"));
    }
}
