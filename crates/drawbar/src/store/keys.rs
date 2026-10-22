//! What drawbar keeps in a library's toshokan root: its keys, where the root is, and
//! the identity toshokan binds files by. Only the desktop opens a library's facts on
//! this build.

use nord_format::formats;
use nord_usb::{Location, ObjectClass};
use serde::{Deserialize, Serialize};
use toshokan::env::{Blake3, Digest};
use toshokan::io::Range;
use toshokan::{Identify, Identity, Register, RelPath, Set};
#[cfg(not(target_arch = "wasm32"))]
use toshokan::{Layout, Schema};

use super::LibPath;
use crate::workspace::Origin;

/// The tags on an asset, by name.
pub const TAGS: Set<String> = Set::new("tags");

/// Every tag made, by name, worn or not: the library's list of tags. Writers that make
/// a list at once each keep one, and the tags are all their names together.
pub const TAG_NAMES: Set<String> = Set::new("tag names");

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

/// The directory at the library's root that holds what is drawbar's own. An older
/// build's index stays there, unread.
#[cfg(not(target_arch = "wasm32"))]
pub const DIR: &str = ".drawbar";

#[cfg(not(target_arch = "wasm32"))]
pub fn schema() -> Schema {
    Schema::of(&[TAGS.key(), TAG_NAMES.key(), ORIGIN.key()]).expect("drawbar's keys are distinct")
}

/// Where toshokan keeps the library's facts, in [`DIR`], and what its scans leave out:
/// every name that starts with a dot, which drawbar never lists.
#[cfg(not(target_arch = "wasm32"))]
pub fn layout() -> Layout {
    Layout::within(DIR, "library")
        .expect("the root's names are single components")
        .hiding()
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

/// Whether the file at `path` is, by its name, a Nord file whose checksum covers it.
fn checksummed(path: &RelPath) -> bool {
    let extension = path.name().and_then(|name| name.rsplit_once('.'));
    let format = extension.and_then(|(_, extension)| formats::by_extension(extension));
    format.is_some_and(|format| nord_format::cbin_formats().any(|tag| tag == format.tag))
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

/// How the volume a library is on compares names: as [`super::names::key`] does on
/// macOS and Windows, whose volumes take two spellings for one name, and byte for byte
/// elsewhere, where a folder can hold both.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy, Debug, Default)]
pub struct Volume;

#[cfg(not(target_arch = "wasm32"))]
impl toshokan::Names for Volume {
    fn key(&self, path: &str) -> String {
        if !cfg!(any(target_vendor = "apple", windows)) {
            return path.to_string();
        }
        let names: Vec<String> = path.split('/').map(super::names::key).collect();
        names.join("/")
    }
}

/// The identity of `bytes` as the library file at `path`, or `None` for a path toshokan
/// does not take.
pub fn identity_of_bytes(path: &LibPath, bytes: &[u8]) -> Option<Identity> {
    let path = RelPath::new(path.as_str()).ok()?;
    Some(Contents.of_bytes(&path, bytes))
}

/// The identity of the library file at `path`, which `file` opens.
#[cfg(not(target_arch = "wasm32"))]
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
        let (upper, lower) = (
            Volume.key("Live/Flu\u{308}gel.NPNO"),
            Volume.key("live/flügel.npno"),
        );
        match cfg!(any(target_vendor = "apple", windows)) {
            true => assert_eq!(upper, lower),
            false => assert_ne!(upper, lower),
        }
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
}
