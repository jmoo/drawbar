//! The unsaved edits of slot views, kept in this install's own state for the library,
//! one file per slot: the hash of the bytes the view was opened over, on a line of its
//! own, then the bytes it holds.

use std::fs::{self, File};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use super::native::sync_dir;
use super::Slot;

/// The folder of this install's state for the library that holds them.
const SLOTS: &str = "slots";

/// What a slot's file is called while it is being written.
const PARTIAL: &str = ".partial";

/// Where the edit of `slot` is kept under `local`.
fn path(local: &Path, slot: &Slot) -> PathBuf {
    let instrument = blake3::hash(slot.instrument.as_bytes());
    let instrument: String = instrument.as_bytes()[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let name = format!("{instrument}-{}-{}-{}", slot.class, slot.bank, slot.slot);
    local.join(SLOTS).join(name)
}

/// Keep `bytes` as the edit of `slot`, made over the slot's bytes of hash `base`. A
/// crash leaves the edit kept before, or this one, whole.
pub fn keep(local: &Path, slot: &Slot, base: blake3::Hash, bytes: &[u8]) -> io::Result<()> {
    let at = path(local, slot);
    let dir = at.parent().expect("a slot's file is in a folder");
    fs::create_dir_all(dir)?;
    let mut partial = at.clone().into_os_string();
    partial.push(PARTIAL);
    let partial = PathBuf::from(partial);
    let mut file = File::create(&partial)?;
    writeln!(file, "{}", base.to_hex())?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(&partial, &at)?;
    sync_dir(dir)
}

/// The edit kept of `slot`, with the hash of the bytes it was made over; `None` where
/// none is.
pub fn read(local: &Path, slot: &Slot) -> io::Result<Option<(blake3::Hash, Vec<u8>)>> {
    let kept = match fs::read(path(local, slot)) {
        Ok(kept) => kept,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let unreadable =
        || io::Error::new(io::ErrorKind::InvalidData, "it is not an edit drawbar kept");
    let end = kept
        .iter()
        .position(|&byte| byte == b'\n')
        .ok_or_else(unreadable)?;
    let (line, bytes) = (&kept[..end], &kept[end + 1..]);
    let base = std::str::from_utf8(line)
        .ok()
        .and_then(|hex| blake3::Hash::from_hex(hex).ok())
        .ok_or_else(unreadable)?;
    Ok(Some((base, bytes.to_vec())))
}

/// Let go of the edit kept of `slot`, if one is.
pub fn discard(local: &Path, slot: &Slot) -> io::Result<()> {
    match fs::remove_file(path(local, slot)) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Temp;

    fn slot(instrument: &str, bank: u32) -> Slot {
        Slot {
            instrument: instrument.to_string(),
            class: 7,
            bank,
            slot: 3,
        }
    }

    #[test]
    fn an_edit_kept_of_a_slot_reads_back_until_it_is_discarded() {
        let local = Temp::new();
        let local = &local.0;
        let base = blake3::hash(b"slot");
        keep(local, &slot("A", 1), base, b"edit\nwith lines").unwrap();
        let kept = read(local, &slot("A", 1)).unwrap();
        assert_eq!(kept, Some((base, b"edit\nwith lines".to_vec())));
        assert_eq!(
            read(local, &slot("B", 1)).unwrap(),
            None,
            "another instrument"
        );
        assert_eq!(read(local, &slot("A", 2)).unwrap(), None, "another bank");
        discard(local, &slot("A", 1)).unwrap();
        assert_eq!(read(local, &slot("A", 1)).unwrap(), None);
        discard(local, &slot("A", 1)).unwrap();
    }

    #[test]
    fn a_slot_file_without_its_hash_does_not_read() {
        let local = Temp::new();
        let local = &local.0;
        let at = path(local, &slot("A", 1));
        fs::create_dir_all(at.parent().unwrap()).unwrap();
        fs::write(&at, b"not a hash\nbytes").unwrap();
        assert!(read(local, &slot("A", 1)).is_err());
    }
}
