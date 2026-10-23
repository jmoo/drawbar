//! Unsaved edits: opaque bytes over a base identity, kept in the local root and
//! never in the folder. Losing the local root loses them and nothing else.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::flow::{self, fold};
use crate::ids::{EntityId, EntryHash, Hlc, Identity};
use crate::io::{Kind, Root, Task};
use crate::layout::Layout;
use crate::path::RelPath;

/// Whose unsaved edit a draft is: an entity's, or the file's at a path no entity is
/// bound to, which an edit alone does not make an entity.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Drafted {
    Entity(EntityId),
    File(RelPath),
}

/// An unsaved edit being changed through `library`, a driver's library.
pub struct Draft<L> {
    library: L,
    of: Drafted,
}

impl<L> Draft<L> {
    pub fn new(library: L, of: Drafted) -> Self {
        Self { library, of }
    }

    pub fn into_parts(self) -> (L, Drafted) {
        (self.library, self.of)
    }
}

/// `drafts/<entity>.json`, or `drafts/file-<hash of the path>.json`, in a writer's
/// local directory. It applies only while the file still holds `base`.
///
/// Written as a JSON header line, `{"base":"<identity>"}` with `"path"` for a file's
/// and `"at"` for when it was kept, then the bytes.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DraftRecord {
    pub base: Identity,
    /// The file's path, for a draft of a file no entity is bound to.
    pub path: Option<RelPath>,
    /// The keeping writer's clock when it was kept, which orders the drafts of
    /// several writers as their entries are ordered, whatever the files' times say.
    pub at: Option<Hlc>,
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
            path: header.path,
            at: header.at,
            bytes: bytes[split + 1..].to_vec(),
        })
    }

    pub fn encode(&self) -> Vec<u8> {
        let header = Header {
            base: self.base,
            path: self.path.clone(),
            at: self.at,
        };
        let mut encoded = serde_json::to_vec(&header).expect("a header serializes");
        encoded.push(b'\n');
        encoded.extend_from_slice(&self.bytes);
        encoded
    }
}

#[derive(Serialize, Deserialize)]
struct Header {
    base: Identity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    path: Option<RelPath>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    at: Option<Hlc>,
}

/// A draft as the local root keeps it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Kept {
    pub of: Drafted,
    /// `None` when it cannot be read.
    pub record: Option<DraftRecord>,
}

/// Where the draft of `of` is kept, for the writer whose genesis entry is `genesis`.
fn kept_at(genesis: EntryHash, of: &Drafted) -> RelPath {
    match of {
        Drafted::Entity(entity) => Layout::draft(genesis, *entity),
        Drafted::File(path) => Layout::file_draft(genesis, path),
    }
}

/// Every draft kept for the writer whose genesis entry is `genesis`, in order of
/// its file's name. A file's draft that does not read is left out, since it does
/// not say whose it is. Reads only the local root.
pub fn read_all(genesis: EntryHash) -> Task<'static, Result<Vec<Kept>>> {
    flow::list(Root::Local, &Layout::drafts(genesis))
        .and_then(move |entries| {
            let stems: BTreeSet<String> = entries
                .into_iter()
                .filter(|entry| entry.kind == Kind::File)
                .filter_map(|entry| {
                    let stem = entry.name.strip_suffix(".next").unwrap_or(&entry.name);
                    Some(stem.strip_suffix(".json")?.to_owned())
                })
                .collect();
            let drafts = Layout::drafts(genesis);
            fold(stems.into_iter(), Vec::new(), move |mut kept, stem| {
                let entity = stem.parse::<EntityId>().ok();
                let path = drafts
                    .join(&format!("{stem}.json"))
                    .expect("a listed name is one component");
                flow::read_replaced(Root::Local, path.clone()).map_ok(move |bytes| {
                    let record = bytes.and_then(|bytes| DraftRecord::decode(&path, &bytes).ok());
                    let (of, record) = match (entity, record) {
                        (Some(entity), record) => (Some(Drafted::Entity(entity)), record),
                        (None, Some(record)) => match record.path.clone() {
                            // A draft under another file's name does not read as this one's.
                            Some(at) if Layout::file_draft(genesis, &at) == path => {
                                (Some(Drafted::File(at)), Some(record))
                            }
                            Some(at) => (Some(Drafted::File(at)), None),
                            None => (None, None),
                        },
                        (None, None) => (None, None),
                    };
                    kept.extend(of.map(|of| Kept { of, record }));
                    kept
                })
            })
        })
        .task()
}

/// Keeps `record` as the draft of `of`, replacing the one kept, so a crash leaves
/// the old draft or the new.
pub fn put(genesis: EntryHash, of: &Drafted, record: &DraftRecord) -> Task<'static, Result<()>> {
    let bytes = record.encode();
    let path = kept_at(genesis, of);
    flow::ensure_dir(Root::Local, &Layout::drafts(genesis))
        .and_then(move |()| flow::replace(Root::Local, path, bytes))
        .task()
}

/// Removes the draft of `of`; none kept is success. A file's draft kept under
/// another file's name, which [`read_all`] reports unreadable, goes too.
pub fn discard(genesis: EntryHash, of: &Drafted) -> Task<'static, Result<()>> {
    let path = kept_at(genesis, of);
    let staged = flow::staged(&path);
    let removed = flow::remove(Root::Local, &staged).and_then({
        let path = path.clone();
        move |()| flow::remove(Root::Local, &path)
    });
    let Drafted::File(at) = of.clone() else {
        return removed.task();
    };
    removed
        .and_then(move |()| misnamed(genesis, at, path))
        .and_then(|misnamed| {
            fold(misnamed.into_iter(), (), |(), path| {
                flow::remove(Root::Local, &path)
            })
        })
        .task()
}

/// The drafts of the file at `at` kept under a name other than `named`.
fn misnamed<'a>(
    genesis: EntryHash,
    at: RelPath,
    named: RelPath,
) -> flow::Fallible<'a, Vec<RelPath>> {
    let drafts = Layout::drafts(genesis);
    flow::list(Root::Local, &drafts).and_then(move |entries| {
        let others: Vec<RelPath> = entries
            .into_iter()
            .filter_map(|entry| {
                let name = entry.name.strip_prefix("file-")?.strip_suffix(".json")?;
                let path = drafts.join(&format!("file-{name}.json")).ok()?;
                (entry.kind == Kind::File && path != named).then_some(path)
            })
            .collect();
        fold(others.into_iter(), Vec::new(), move |mut misnamed, path| {
            let at = at.clone();
            flow::read_replaced(Root::Local, path.clone()).map_ok(move |bytes| {
                let record = bytes.and_then(|bytes| DraftRecord::decode(&path, &bytes).ok());
                if record.is_some_and(|record| record.path.as_ref() == Some(&at)) {
                    misnamed.push(path);
                }
                misnamed
            })
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_draft_keeps_its_bytes_verbatim_after_a_header_line() {
        let draft = DraftRecord {
            base: Identity::from_u128(0xab),
            path: None,
            at: None,
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
