//! What drawbar keeps in a library's toshokan root: its keys, where the root is, and
//! the identity toshokan binds files by. Only the desktop opens a library's facts on
//! this build.

#[cfg(not(target_arch = "wasm32"))]
use toshokan::env::{Blake3, Digest};
#[cfg(not(target_arch = "wasm32"))]
use toshokan::io::Range;
use toshokan::Set;
#[cfg(not(target_arch = "wasm32"))]
use toshokan::{Identify, Identity, Layout, RelPath, Schema};

/// The tags on an asset, by name.
pub const TAGS: Set<String> = Set::new("tags");

/// The directory at the library's root that holds what is drawbar's own. An older
/// build's index stays there, unread.
#[cfg(not(target_arch = "wasm32"))]
pub const DIR: &str = ".drawbar";

#[cfg(not(target_arch = "wasm32"))]
pub fn schema() -> Schema {
    Schema::of(&[TAGS.key()]).expect("drawbar's keys are distinct")
}

#[cfg(not(target_arch = "wasm32"))]
pub fn layout() -> Layout {
    Layout::within(DIR, "library").expect("the root's names are single components")
}

/// How many bytes at the start of a file its identity reads.
#[cfg(not(target_arch = "wasm32"))]
const HEAD: u64 = 64 << 10;
/// How many bytes at the end of a file its identity reads.
#[cfg(not(target_arch = "wasm32"))]
const TAIL: u64 = 2;

/// A file's identity: BLAKE3 over its length, its first 64 KiB and its last two bytes.
/// A whole Nord file's checksum lies within those: a type-1 header holds the body's
/// CRC-32 at `0x18`, and a type-0 file ends with the CRC-16 of every byte before it. A
/// change anywhere in such a file changes its identity, however large the file is.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy, Debug, Default)]
pub struct Contents;

#[cfg(not(target_arch = "wasm32"))]
impl Identify for Contents {
    fn ranges(&self, _: &RelPath, len: u64) -> Vec<Range> {
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

    fn digest(&self, _: &RelPath, len: u64) -> Box<dyn Digest> {
        let mut hasher = blake3::Hasher::new();
        hasher.update(&len.to_le_bytes());
        Box::new(Blake3(hasher))
    }
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

    fn identity(bytes: &[u8]) -> Identity {
        Contents.of_bytes(&RelPath::ROOT, bytes)
    }

    #[test]
    fn an_identity_reads_the_head_and_the_last_two_bytes_of_a_large_file() {
        let ranges = Contents.ranges(&RelPath::ROOT, 10 << 20);
        let read: Vec<(u64, u64)> = ranges.iter().map(|r| (r.offset, r.len)).collect();
        assert_eq!(read, [(0, 64 << 10), ((10 << 20) - 2, 2)]);
        let short: Vec<(u64, u64)> = Contents
            .ranges(&RelPath::ROOT, 1)
            .iter()
            .map(|r| (r.offset, r.len))
            .collect();
        assert_eq!(short, [(0, 1), (0, 1)]);
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
        assert_ne!(identity(&piano), identity(&edited));
        assert_eq!(identity(&piano), identity(&piano.clone()));
    }

    #[test]
    fn files_of_one_head_and_tail_differ_by_length() {
        let a = vec![7; 70_000];
        let b = vec![7; 70_001];
        assert_ne!(identity(&a), identity(&b));
    }

    #[test]
    fn an_identity_read_from_a_file_is_the_one_of_its_bytes() {
        let dir = crate::testing::Temp::new();
        let bytes = crate::testing::wav_bytes();
        std::fs::write(dir.at("a.wav"), &bytes).unwrap();
        let mut file = std::fs::File::open(dir.at("a.wav")).unwrap();
        let path = RelPath::new("a.wav").unwrap();
        assert_eq!(identity_of(&path, &mut file).unwrap(), identity(&bytes));
    }
}
