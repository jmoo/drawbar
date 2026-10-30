//! What drawbar keeps in a library's toshokan root: its keys, where the root is, and
//! the identity toshokan binds files by.

use nord_usb::{Location, ObjectClass};
use serde::{Deserialize, Serialize};
use toshokan::env::{Blake3, Digest};
use toshokan::io::Range;
use toshokan::{Identify, Identity, Layout, Register, RelPath, Schema, Set};

use super::LibPath;
use crate::workspace::Origin;

/// The tags on an asset, by name.
pub const TAGS: Set<String> = Set::new("tags");

/// Every tag made, by name, worn or not: the library's list of tags. Writers that make
/// a list at once each keep one, and the tags are all their names together.
pub const TAG_NAMES: Set<String> = Set::new("tag names");

/// The indexes older drawbars kept in the folder, `.drawbar/library.ron`, whose tags and
/// origins the library took in, by the BLAKE3 of their bytes: each is taken in once.
pub const TAKEN_IN: Set<String> = Set::new("taken in");

/// Where an asset came from, where its file cannot say: made new in drawbar, copied off
/// a slot, or the occupant a write to a slot could not put back. A file opened from
/// anywhere else says nothing, and has no entity for it.
pub const ORIGIN: Register<Came> = Register::new("origin");

/// An [`ORIGIN`], as the library keeps it. A slot is its class's code and its bank and
/// slot from zero, as the wire numbers them.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Serialize, Deserialize)]
#[serde(tag = "from", rename_all = "lowercase", deny_unknown_fields)]
pub enum Came {
    New,
    Slot { class: u32, bank: u32, slot: u32 },
    Rescued { bank: u32, slot: u32 },
}

impl Came {
    /// What the library keeps of `origin`, where it keeps anything.
    pub fn of(origin: &Origin) -> Option<Came> {
        match origin {
            Origin::File(_) => None,
            Origin::Fresh => Some(Came::New),
            Origin::Device { class, at } => Some(Came::Slot {
                class: class.to_raw(),
                bank: at.bank,
                slot: at.slot,
            }),
            Origin::Rescued { at } => Some(Came::Rescued {
                bank: at.bank,
                slot: at.slot,
            }),
        }
    }

    /// The origin this says.
    pub fn origin(self) -> Origin {
        match self {
            Came::New => Origin::Fresh,
            Came::Slot { class, bank, slot } => Origin::Device {
                class: ObjectClass::from_raw(class),
                at: Location { bank, slot },
            },
            Came::Rescued { bank, slot } => Origin::Rescued {
                at: Location { bank, slot },
            },
        }
    }
}

/// How many bytes at the start of a Nord file its identity reads.
const HEAD: u64 = 64 << 10;
/// How many bytes at the end of a Nord file its identity reads.
const TAIL: u64 = 2;

/// A file's identity: BLAKE3 over its length and its bytes. A Nord file in its CBIN
/// container is read only at its first 64 KiB and its last two bytes, which hold its
/// checksum: a type-1 header holds the body's CRC-32 at `0x18`, and a type-0 file ends
/// with the CRC-16 of every byte before it. Every other file is read whole.
///
/// ⚠️ The two agree only while a Nord file's checksum is its contents', as every Nord
/// application and drawbar keep it: an edit that leaves the checksum stale keeps the
/// identity.
#[derive(Clone, Copy, Debug, Default)]
pub struct Contents;

/// The extensions of the files whose identity reads only their head and their checksum:
/// the CBIN formats nord-format read when the library's format was set.
///
/// ⚠️ Part of the library's format: an identity is how a file is known across renames and
/// computers, so a name added or removed here makes every tagged file of that extension
/// look changed outside drawbar. A CBIN format added since is read whole.
const CHECKSUMMED: [&str; 75] = [
    "nc2p", "nc2s", "ncpg", "ncsy", "nd2p", "nd3k", "ne4l", "ne4p", "ne4s", "ne5l", "ne5p", "ne5s",
    "ne5t", "ne6l", "ne6p", "ne6t", "ne7l", "ne7p", "ne7t", "neop", "nepg", "ng2l", "ng2p", "ng2t",
    "nl4p", "nl4s", "nl4t", "nlap", "nlas", "nlat", "no3p", "no3t", "np2l", "np2p", "np2s", "np3l",
    "np3p", "np3s", "np4l", "np4p", "np4t", "np5l", "np5p", "np5t", "npip", "npli", "npno", "nppg",
    "npsy", "ns2l", "ns2p", "ns2s", "ns2y", "ns3f", "ns3l", "ns3s", "ns3t", "ns3y", "ns4l", "ns4n",
    "ns4o", "ns4p", "ns4t", "ns4y", "nsmp", "nsmp3", "nsmp4", "nsp", "nspg", "nss", "nw2l", "nw2p",
    "nw2s", "nwp", "nwsy",
];

/// Whether the file at `path` is, by its name, a Nord file whose checksum covers it.
fn checksummed(path: &RelPath) -> bool {
    let extension = path.name().and_then(|name| name.rsplit_once('.'));
    extension.is_some_and(|(_, extension)| {
        CHECKSUMMED
            .iter()
            .any(|known| known.eq_ignore_ascii_case(extension))
    })
}

impl Identify for Contents {
    fn ranges(&self, path: &RelPath, len: u64) -> Vec<Range> {
        if !checksummed(path) {
            return vec![Range { offset: 0, len }];
        }
        vec![
            Range {
                offset: 0,
                len: len.min(HEAD),
            },
            Range {
                offset: len.saturating_sub(TAIL),
                len: len.min(TAIL),
            },
        ]
    }

    fn digest(&self, path: &RelPath, len: u64) -> Box<dyn Digest> {
        let mut hasher = blake3::Hasher::new();
        hasher.update(&[u8::from(checksummed(path))]);
        hasher.update(&len.to_le_bytes());
        Box::new(Blake3(hasher))
    }
}

/// The identity of `bytes` as the library file at `path`, or `None` for a path toshokan
/// does not take.
pub fn identity_of_bytes(path: &LibPath, bytes: &[u8]) -> Option<Identity> {
    let path = RelPath::new(path.as_str()).ok()?;
    Some(Contents.of_bytes(&path, bytes))
}

/// The directory at the library's root that holds what is drawbar's own. An older
/// build's index stays there, unread.
pub const DIR: &str = ".drawbar";

/// The directory of [`DIR`] that holds each install's own state, in a browser's own
/// library, which no copy of the library takes.
pub const LOCAL: &str = "local";

pub fn schema() -> Schema {
    Schema::of(&[TAGS.key(), TAG_NAMES.key(), ORIGIN.key(), TAKEN_IN.key()])
        .expect("drawbar's keys are distinct")
}

/// Where toshokan keeps the library's facts, in [`DIR`], and what its scans leave out:
/// every name that starts with a dot, which drawbar never lists.
pub fn layout() -> Layout {
    Layout::within(DIR, "library")
        .expect("the root's names are single components")
        .hiding()
}

/// How the volume a library is on compares names: ignoring case where a look at
/// the volume says it does, and in one normalization form on macOS, whose volumes
/// take two spellings of an accent for one name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Volume {
    pub folds_case: bool,
    pub folds_form: bool,
}

/// `name` with each letter's case swapped.
pub fn swapped(name: &str) -> String {
    name.chars()
        .flat_map(|c| match c.is_uppercase() {
            true => c.to_lowercase().collect::<Vec<_>>(),
            false => c.to_uppercase().collect::<Vec<_>>(),
        })
        .collect()
}

impl toshokan::Names for Volume {
    fn key(&self, path: &str) -> String {
        use unicode_normalization::UnicodeNormalization as _;
        let name = |name: &str| {
            let name = match self.folds_case {
                true => name.to_lowercase(),
                false => name.to_string(),
            };
            match self.folds_form {
                true => name.nfc().collect(),
                false => name,
            }
        };
        let names: Vec<String> = path.split('/').map(name).collect();
        names.join("/")
    }
}

/// What only the desktop, which reads a volume's names itself, needs.
#[cfg(not(target_arch = "wasm32"))]
mod desktop {
    use toshokan::{Identify, Identity, RelPath};

    use super::{swapped, Contents, Volume};

    impl Volume {
        /// How the volume holding `folder` compares names, as a look at it, which
        /// writes nothing, finds: a name's other case reaching the same entry. Where
        /// nothing there has a name with a case to swap, as the system's volumes
        /// usually do: ignoring case on macOS and Windows.
        pub fn probe(folder: &std::path::Path) -> Volume {
            let usual = cfg!(any(target_vendor = "apple", windows));
            Volume {
                folds_case: folds_case(folder).unwrap_or(usual),
                folds_form: cfg!(target_vendor = "apple"),
            }
        }
    }

    /// Whether the volume holding `folder` takes a name in another case for the same
    /// entry, by the first name with a case to swap. `None` where no name has one.
    fn folds_case(folder: &std::path::Path) -> Option<bool> {
        let (dir, twin) = probed(folder)?;
        let reached = std::fs::symlink_metadata(dir.join(&twin)).is_ok();
        let listed = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .any(|entry| entry.file_name().to_str() == Some(twin.as_str()));
        Some(reached && !listed)
    }

    /// The directory and the name, case swapped, the volume of `folder` is probed by:
    /// of a name in it, else of the folder's own, which is on another volume where the
    /// folder is where one is mounted.
    pub(super) fn probed(folder: &std::path::Path) -> Option<(std::path::PathBuf, String)> {
        let own = folder
            .parent()
            .zip(folder.file_name().and_then(|name| name.to_str()))
            .map(|(dir, name)| (dir.to_path_buf(), name.to_string()));
        let inside = std::fs::read_dir(folder).into_iter().flatten().flatten();
        let inside = inside.filter_map(|entry| {
            Some((folder.to_path_buf(), entry.file_name().into_string().ok()?))
        });
        inside.take(64).chain(own).find_map(|(dir, name)| {
            let twin = swapped(&name);
            (twin != name).then_some((dir, twin))
        })
    }

    /// The identity of the library file at `path`, which `file` opens.
    pub fn identity_of(path: &RelPath, file: &mut std::fs::File) -> std::io::Result<Identity> {
        use std::io::{Read as _, Seek as _, SeekFrom};

        let len = file.metadata()?.len();
        let mut digest = Contents.digest(path, len);
        for range in Contents.ranges(path, len) {
            file.seek(SeekFrom::Start(range.offset))?;
            let mut part = Vec::new();
            file.by_ref().take(range.len).read_to_end(&mut part)?;
            digest.update(&part);
        }
        Ok(digest.finish())
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use desktop::*;

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> RelPath {
        RelPath::new(text).unwrap()
    }

    fn identity(path: &str, bytes: &[u8]) -> Identity {
        Contents.of_bytes(&at(path), bytes)
    }

    fn read(path: &str, len: u64) -> Vec<(u64, u64)> {
        let ranges = Contents.ranges(&at(path), len);
        ranges
            .iter()
            .map(|range| (range.offset, range.len))
            .collect()
    }

    #[test]
    fn a_nord_file_is_read_at_its_head_and_checksum_and_any_other_whole() {
        let big = 10 << 20;
        assert_eq!(read("Grand.npno", big), [(0, HEAD), (big - 2, 2)]);
        assert_eq!(read("Live/A.NE5P", big), [(0, HEAD), (big - 2, 2)]);
        assert_eq!(read("Cello.wav", big), [(0, big)]);
        assert_eq!(read("notes.pdf", big), [(0, big)]);
        assert_eq!(read("Grand", big), [(0, big)]);
        assert_eq!(read("Grand.npno", 1), [(0, 1), (0, 1)]);
    }

    #[test]
    fn a_nord_file_edited_past_the_head_has_another_identity() {
        let piano = crate::testing::piano(64);
        assert!(piano.len() as u64 > HEAD, "{} bytes", piano.len());
        let at = piano.len() - 100;
        let mut edited = piano.clone();
        edited[at] ^= 0xff;
        let body = 0x2c..edited.len();
        let crc = nord_format::crc::crc32(&edited[body]);
        edited[0x18..0x1c].copy_from_slice(&crc.to_le_bytes());
        assert_ne!(
            identity("Grand.npno", &piano),
            identity("Grand.npno", &edited)
        );
        assert_eq!(
            identity("Grand.npno", &piano),
            identity("Grand.npno", &piano)
        );
    }

    #[test]
    fn a_change_anywhere_in_a_file_of_another_kind_changes_its_identity() {
        let mut wav = crate::testing::wav_bytes();
        wav.resize(200_000, 7);
        for path in ["Cello.wav", "notes.pdf"] {
            for at in [0, 70_000, 150_000, 199_997] {
                let mut edited = wav.clone();
                edited[at] ^= 1;
                assert_ne!(
                    identity(path, &wav),
                    identity(path, &edited),
                    "{path} at {at}"
                );
            }
        }
    }

    #[test]
    fn files_of_one_head_and_tail_differ_by_length() {
        let a = vec![7; 70_000];
        let b = vec![7; 70_001];
        assert_ne!(identity("a.npno", &a), identity("a.npno", &b));
    }

    #[test]
    fn an_origin_the_library_keeps_comes_back_as_it_was() {
        let at = Location::from_user(4, 2);
        for origin in [
            Origin::Fresh,
            Origin::Device {
                class: ObjectClass::Sample,
                at,
            },
            Origin::Rescued { at },
        ] {
            let came = Came::of(&origin).expect("kept");
            assert_eq!(came.origin().label(), origin.label());
        }
        assert_eq!(Came::of(&Origin::File("a.ne5p".into())), None);
    }

    #[test]
    fn names_compare_as_the_volume_compares_them() {
        use toshokan::Names as _;
        let (upper, lower) = ("Live/Flu\u{308}gel.NPNO", "live/flügel.npno");
        let folding = Volume {
            folds_case: true,
            folds_form: true,
        };
        assert_eq!(folding.key(upper), folding.key(lower));
        let exact = Volume {
            folds_case: false,
            folds_form: false,
        };
        assert_ne!(exact.key(upper), exact.key(lower));
    }

    /// The probe agrees with what the volume does with a name in another case.
    #[test]
    fn a_volume_is_probed_for_folding_case_as_it_folds_it() {
        let dir = crate::testing::Temp::new();
        std::fs::write(dir.at("probe"), b"").unwrap();
        let folds = dir.at("PROBE").exists();
        assert_eq!(Volume::probe(&dir.0).folds_case, folds);
    }

    #[test]
    fn a_volume_is_probed_by_a_name_in_the_folder_before_its_own() {
        let dir = crate::testing::Temp::new();
        std::fs::write(dir.at("probe"), b"").unwrap();
        assert_eq!(
            desktop::probed(&dir.0),
            Some((dir.0.clone(), "PROBE".to_string()))
        );
    }

    #[test]
    fn an_identity_read_from_a_file_is_the_one_of_its_bytes() {
        let dir = crate::testing::Temp::new();
        let bytes = crate::testing::wav_bytes();
        std::fs::write(dir.at("a.wav"), &bytes).unwrap();
        let mut file = std::fs::File::open(dir.at("a.wav")).unwrap();
        let path = at("a.wav");
        assert_eq!(
            identity_of(&path, &mut file).unwrap(),
            identity("a.wav", &bytes)
        );
    }
    #[test]
    fn every_checksummed_extension_is_still_a_cbin_format() {
        for extension in CHECKSUMMED {
            let format = nord_format::formats::by_extension(extension);
            let cbin = format.is_some_and(|f| nord_format::cbin_formats().any(|t| t == f.tag));
            assert!(cbin, "{extension}");
        }
    }
}
