//! Unsaved edits: opaque bytes over a base identity, kept in the local root and
//! never in the folder. Losing the local root loses them and nothing else.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::flow::{self, fold};
use crate::ids::{EntityId, EntryHash, Identity};
use crate::io::{Kind, Root, Task};
use crate::layout::Layout;
use crate::path::RelPath;

/// An entity's unsaved edit being changed through `library`, a driver's library.
pub struct Draft<L> {
    library: L,
    entity: EntityId,
}

impl<L> Draft<L> {
    pub fn new(library: L, entity: EntityId) -> Self {
        Self { library, entity }
    }

    pub fn into_parts(self) -> (L, EntityId) {
        (self.library, self.entity)
    }
}

/// `drafts/<entity>.json` in a writer's local directory. It applies only while the
/// entity's file still holds `base`.
///
/// Written as a JSON header line, `{"base":"<identity>"}`, then the bytes.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DraftRecord {
    pub base: Identity,
    pub bytes: Vec<u8>,
}

impl DraftRecord {
    pub fn decode(path: &RelPath, bytes: &[u8]) -> Result<Self> {
        let corrupt = |reason: String| Error::Corrupt {
            path: path.clone(),
            reason,
        };
        let split = bytes
            .iter()
            .position(|&b| b == b'\n')
            .ok_or_else(|| corrupt("no header line".into()))?;
        let header: Header =
            serde_json::from_slice(&bytes[..split]).map_err(|e| corrupt(e.to_string()))?;
        Ok(Self {
            base: header.base,
            bytes: bytes[split + 1..].to_vec(),
        })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut encoded =
            serde_json::to_vec(&Header { base: self.base }).expect("a header serializes");
        encoded.push(b'\n');
        encoded.extend_from_slice(&self.bytes);
        encoded
    }
}

#[derive(Serialize, Deserialize)]
struct Header {
    base: Identity,
}

/// A draft as the local root keeps it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Kept {
    pub entity: EntityId,
    /// `None` when it cannot be read.
    pub record: Option<DraftRecord>,
}

/// Every draft kept for the writer whose genesis entry is `genesis`, by entity.
/// Reads only the local root.
pub fn read_all(genesis: EntryHash) -> Task<'static, Result<Vec<Kept>>> {
    flow::list(Root::Local, &Layout::drafts(genesis))
        .and_then(move |entries| {
            let entities: BTreeSet<EntityId> = entries
                .into_iter()
                .filter(|entry| entry.kind == Kind::File)
                .filter_map(|entry| {
                    let stem = entry.name.strip_suffix(".next").unwrap_or(&entry.name);
                    stem.strip_suffix(".json")?.parse().ok()
                })
                .collect();
            fold(entities.into_iter(), Vec::new(), move |mut drafts, entity| {
                let path = Layout::draft(genesis, entity);
                flow::read_replaced(Root::Local, path.clone()).map_ok(move |bytes| {
                    let record = bytes.and_then(|bytes| DraftRecord::decode(&path, &bytes).ok());
                    drafts.push(Kept { entity, record });
                    drafts
                })
            })
        })
        .task()
}

/// Keeps `record` as `entity`'s draft, replacing the one kept, so a crash leaves
/// the old draft or the new.
pub fn put(genesis: EntryHash, entity: EntityId, record: &DraftRecord) -> Task<'static, Result<()>> {
    let bytes = record.encode();
    flow::ensure_dir(Root::Local, &Layout::drafts(genesis))
        .and_then(move |()| flow::replace(Root::Local, Layout::draft(genesis, entity), bytes))
        .task()
}

/// Removes `entity`'s draft; none kept is success.
pub fn discard(genesis: EntryHash, entity: EntityId) -> Task<'static, Result<()>> {
    let path = Layout::draft(genesis, entity);
    let staged = flow::staged(&path);
    flow::remove(Root::Local, &staged)
        .and_then(move |()| flow::remove(Root::Local, &path))
        .task()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_draft_keeps_its_bytes_verbatim_after_a_header_line() {
        let draft = DraftRecord {
            base: Identity::from_u128(0xab),
            bytes: b"\n{\"base\":1}\n\0".to_vec(),
        };
        let encoded = draft.encode();
        assert!(encoded.starts_with(format!("{{\"base\":\"{}\"}}\n", draft.base).as_bytes()));
        let path = RelPath::new("d.json").unwrap();
        assert_eq!(DraftRecord::decode(&path, &encoded).unwrap(), draft);
    }

    #[test]
    fn a_draft_without_its_header_is_corrupt() {
        let path = RelPath::new("d.json").unwrap();
        for bytes in [&b""[..], b"{}", b"{}\nx", b"{\"base\":\"AB\"}\n"] {
            assert!(
                matches!(
                    DraftRecord::decode(&path, bytes),
                    Err(Error::Corrupt { .. })
                ),
                "{bytes:?}"
            );
        }
    }
}
